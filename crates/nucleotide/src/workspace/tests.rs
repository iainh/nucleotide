use super::documentation::DOC_SIDEBAR_MAX_WIDTH;
use super::global_search::{
    GlobalSearchMatch, global_search_custom_ignore_filenames,
    global_search_disk_matches_with_backend_async, global_search_matches,
};
use super::image_viewer::{remote_image_cache_dir, remote_image_cache_path};
use super::pickers::{
    FILE_PICKER_PREVIEW_MAX_BYTES, code_action_enabled, code_action_metadata_label,
    file_picker_decode_preview_text, file_picker_items_from_backend,
    file_picker_preview_text_from_backend, file_picker_preview_text_from_backend_async,
    file_picker_search_result_from_backend, sort_code_actions_like_helix,
};
#[cfg(target_os = "windows")]
use super::recent_projects::{
    WINDOWS_JUMP_LIST_PROJECT_LIMIT, windows_jump_list_entries, windows_recent_project_path,
    windows_update_recent_project_list, windows_wide_nul_path,
};
use super::status_bar::{
    StatusBarDensity, StatusBarGeometry, abbreviated_vcs_ref, latest_background_activity,
    shorten_statusbar_text, statusbar_file_label, statusbar_language_label,
    statusbar_line_ending_label, statusbar_lsp_summary_for_state, statusbar_notification_icon,
    statusbar_show_secondary_document_metadata,
};
#[allow(unused_imports)]
use super::*;
use helix_core::{Range, Rope, SmallVec};
use nucleotide_lsp::LspStatusKind;
use nucleotide_ui::notification::StatusBarNotificationSeverity;
use slotmap::KeyData;
#[cfg(target_os = "windows")]
use smallvec::smallvec;
#[cfg(target_os = "windows")]
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

fn test_regex(pattern: &str) -> helix_stdx::rope::Regex {
    helix_stdx::rope::RegexBuilder::new()
        .syntax(helix_stdx::rope::Config::new().multi_line(true))
        .build(pattern)
        .unwrap()
}

fn default_file_picker_config() -> helix_view::editor::FilePickerConfig {
    helix_view::editor::Config::default().file_picker
}

#[test]
fn tab_bar_document_cache_reuses_only_matching_presentations() {
    let key = TabBarDocumentCacheKey {
        project_directory: Some(PathBuf::from("/workspace")),
        show_file_icons: true,
        show_git_status: true,
        show_diagnostics: crate::config::TabDiagnosticsVisibility::All,
        show_preview_tabs: true,
        workspace_is_local: true,
    };
    let documents: Arc<[crate::tab_bar::DocumentInfo]> = Arc::from(Vec::new());
    let cache = TabBarDocumentCache {
        generation: 7,
        key: key.clone(),
        documents: documents.clone(),
    };

    let reused = cached_tab_bar_documents(Some(&cache), 7, &key).expect("cache hit");
    assert!(Arc::ptr_eq(&reused, &documents));
    assert!(cached_tab_bar_documents(Some(&cache), 8, &key).is_none());

    let mut changed_key = key;
    changed_key.show_git_status = false;
    assert!(cached_tab_bar_documents(Some(&cache), 7, &changed_key).is_none());
}

#[test]
fn buffer_close_aliases_preserve_force() {
    for command in ["close", "bc", "bclose", "buffer-close"] {
        assert_eq!(buffer_close_command_force(command), Some(false));
    }
    for command in ["close!", "bc !", "bclose!", "buffer-close !"] {
        assert_eq!(buffer_close_command_force(command), Some(true));
    }
    assert_eq!(buffer_close_command_force("buffer-close other"), None);
    assert_eq!(buffer_close_command_force("quit"), None);
}

#[test]
fn helix_command_aliases_are_normalized() {
    assert_eq!(normalized_helix_command("42"), "goto 42");
    assert_eq!(normalized_helix_command("split"), "hsplit");
    assert_eq!(normalized_helix_command("sp"), "hsplit");
    assert_eq!(normalized_helix_command("vs"), "vsplit");
    assert_eq!(
        normalized_helix_command("open 'two words'"),
        "open 'two words'"
    );
}

#[test]
fn remote_document_reload_decision_reloads_visible_clean_match() {
    let path = Path::new("/remote/project/src/lib.rs");
    let docs = [(1_u8, Some(path), false)];
    let views = [(7_u8, 1_u8)];

    assert_eq!(
        remote_document_reload_decision(docs, views, path),
        RemoteDocumentReloadDecision::Reload {
            doc_id: 1,
            view_id: 7,
        }
    );
}

#[test]
fn remote_document_reload_decision_reports_dirty_match() {
    let path = Path::new("/remote/project/src/lib.rs");
    let docs = [(1_u8, Some(path), true)];
    let views = [(7_u8, 1_u8)];

    assert_eq!(
        remote_document_reload_decision(docs, views, path),
        RemoteDocumentReloadDecision::Dirty { doc_id: 1 }
    );
}

#[test]
fn remote_document_reload_decision_reports_hidden_match() {
    let path = Path::new("/remote/project/src/lib.rs");
    let docs = [(1_u8, Some(path), false)];
    let views = [(7_u8, 2_u8)];

    assert_eq!(
        remote_document_reload_decision(docs, views, path),
        RemoteDocumentReloadDecision::Hidden { doc_id: 1 }
    );
}

#[test]
fn remote_document_reload_decision_ignores_unmatched_paths() {
    let path = Path::new("/remote/project/src/lib.rs");
    let other = Path::new("/remote/project/src/main.rs");
    let docs = [(1_u8, Some(other), false), (2_u8, None, false)];
    let views = [(7_u8, 1_u8)];

    assert_eq!(
        remote_document_reload_decision(docs, views, path),
        RemoteDocumentReloadDecision::NoMatch
    );
}

#[test]
fn trash_delete_preference_degrades_to_confirmed_permanent_delete() {
    let remote_identity =
        WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
            kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
            name: "example.test".to_string(),
        });

    let mode = effective_delete_mode(crate::config::DeleteBehavior::Trash, &remote_identity);

    assert_eq!(mode, nucleotide_events::workspace::DeleteMode::Permanent);
    assert!(delete_confirmation_required(mode));
    assert_eq!(delete_confirmation_label(mode), "Delete Permanently");
}

#[test]
fn delete_confirmation_message_matches_effective_mode() {
    let path = Path::new("/remote/project/src/lib.rs");

    assert_eq!(
        delete_confirmation_message(
            Some(path),
            nucleotide_events::workspace::DeleteMode::Permanent,
        ),
        "Delete 'lib.rs' permanently?"
    );
    assert_eq!(
        delete_confirmation_message(Some(path), nucleotide_events::workspace::DeleteMode::Trash,),
        "Move 'lib.rs' to Trash?"
    );
}

fn test_code_action(
    title: &str,
    kind: Option<lsp::CodeActionKind>,
    fixes_diagnostic: bool,
    is_preferred: bool,
    disabled: bool,
) -> lsp::CodeActionOrCommand {
    lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
        title: title.to_string(),
        kind,
        diagnostics: fixes_diagnostic.then(|| {
            vec![lsp::Diagnostic::new_simple(
                lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 1)),
                "diagnostic".to_string(),
            )]
        }),
        edit: None,
        command: None,
        is_preferred: is_preferred.then_some(true),
        disabled: disabled.then(|| lsp::CodeActionDisabled {
            reason: "disabled".to_string(),
        }),
        data: None,
    })
}

fn code_action_title(action: &lsp::CodeActionOrCommand) -> &str {
    match action {
        lsp::CodeActionOrCommand::Command(command) => &command.title,
        lsp::CodeActionOrCommand::CodeAction(code_action) => &code_action.title,
    }
}

#[test]
fn environment_badge_appears_for_native_flake_marker_only() {
    assert_eq!(
        EnvironmentBadge::from_environment_marker(Some("native-flake")),
        Some(EnvironmentBadge::NativeFlake)
    );
    assert_eq!(EnvironmentBadge::from_environment_marker(Some("cli")), None);
    assert_eq!(
        EnvironmentBadge::from_environment_marker(Some("worktree-shell")),
        None
    );
    assert_eq!(EnvironmentBadge::from_environment_marker(None), None);

    let native_flake_snapshot = ProjectEnvironmentSnapshot {
        root: PathBuf::from("/project"),
        variables: std::collections::BTreeMap::new(),
        origin: ProjectEnvironmentOrigin::NativeFlake,
        diagnostics: Vec::new(),
    };
    assert_eq!(
        EnvironmentBadge::from_project_environment_snapshot(&native_flake_snapshot),
        Some(EnvironmentBadge::NativeFlake)
    );

    let mut marker_variables = std::collections::BTreeMap::new();
    marker_variables.insert("ZED_ENVIRONMENT".to_string(), "native-flake".to_string());
    let marker_snapshot = ProjectEnvironmentSnapshot {
        root: PathBuf::from("/project"),
        variables: marker_variables,
        origin: ProjectEnvironmentOrigin::ProcessBaseline,
        diagnostics: Vec::new(),
    };
    assert_eq!(
        EnvironmentBadge::from_project_environment_snapshot(&marker_snapshot),
        Some(EnvironmentBadge::NativeFlake)
    );

    let baseline_snapshot = ProjectEnvironmentSnapshot {
        root: PathBuf::from("/project"),
        variables: std::collections::BTreeMap::new(),
        origin: ProjectEnvironmentOrigin::ProcessBaseline,
        diagnostics: Vec::new(),
    };
    assert_eq!(
        EnvironmentBadge::from_project_environment_snapshot(&baseline_snapshot),
        None
    );
}

#[test]
fn context_menu_target_parent_uses_entry_kind_without_filesystem_probe() {
    assert_eq!(
        context_menu_target_parent_path(Path::new("/workspace/src"), true),
        PathBuf::from("/workspace/src")
    );
    assert_eq!(
        context_menu_target_parent_path(Path::new("/workspace/src/main.rs"), false),
        PathBuf::from("/workspace/src")
    );
    assert_eq!(
        context_menu_target_parent_path(Path::new("main.rs"), false),
        PathBuf::from(".")
    );
}

#[test]
fn workspace_selection_target_maps_backend_file_kinds() {
    assert_eq!(
        workspace_selection_target_from_file_kind(FileKind::File),
        Some(WorkspaceSelectionTarget::File)
    );
    assert_eq!(
        workspace_selection_target_from_file_kind(FileKind::Symlink),
        Some(WorkspaceSelectionTarget::File)
    );
    assert_eq!(
        workspace_selection_target_from_file_kind(FileKind::Directory),
        Some(WorkspaceSelectionTarget::Directory)
    );
    assert_eq!(
        workspace_selection_target_from_file_kind(FileKind::Other),
        None
    );
}

#[tokio::test]
async fn workspace_selection_target_async_classifies_local_paths() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("main.rs");
    std::fs::write(&file_path, "fn main() {}\n").unwrap();

    assert_eq!(
        workspace_selection_target_for_path_async(local_workspace_backend(), file_path).await,
        Some(WorkspaceSelectionTarget::File)
    );
    assert_eq!(
        workspace_selection_target_for_path_async(
            local_workspace_backend(),
            temp_dir.path().to_path_buf()
        )
        .await,
        Some(WorkspaceSelectionTarget::Directory)
    );
}

#[test]
fn titlebar_filename_uses_focused_file_or_app_name() {
    assert_eq!(titlebar_filename(Some("main.rs")), "Nucleotide");
    assert_eq!(titlebar_filename(Some("")), "Nucleotide");
    assert_eq!(titlebar_filename(None), "Nucleotide");
}

#[test]
fn image_viewer_opens_image_paths_for_local_and_remote_backends() {
    let path = Path::new("/project/assets/logo.png");

    assert!(should_open_with_image_viewer(
        path,
        false,
        &WorkspaceIdentity::Local
    ));
    assert!(should_open_with_image_viewer(
        path,
        false,
        &WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
            kind: nucleotide_workspace::RemoteWorkspaceKind::Wsl,
            name: "Ubuntu".to_string(),
        })
    ));
}

#[test]
fn image_viewer_ignores_non_images_and_positioned_opens() {
    assert!(!should_open_with_image_viewer(
        Path::new("/project/src/main.rs"),
        false,
        &WorkspaceIdentity::Local
    ));
    assert!(!should_open_with_image_viewer(
        Path::new("/project/assets/logo.png"),
        true,
        &WorkspaceIdentity::Local
    ));
}

#[test]
fn remote_image_cache_paths_preserve_extension_and_escape_names() {
    let path = Path::new("ssh://devbox/home/me/project/assets/logo one.png");
    let identity = WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
        kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let cache_path = remote_image_cache_path(&identity, path);
    let file_name = cache_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap();

    assert!(cache_path.starts_with(remote_image_cache_dir()));
    assert!(file_name.ends_with("-logo_one.png"));
}

#[test]
fn remote_image_cache_paths_are_scoped_by_remote_identity() {
    let path = Path::new("/home/me/project/assets/logo.png");
    let first = WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
        kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
        name: "devbox-a".to_string(),
    });
    let second = WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
        kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
        name: "devbox-b".to_string(),
    });

    assert_ne!(
        remote_image_cache_path(&first, path),
        remote_image_cache_path(&second, path)
    );
}

#[test]
fn recent_project_integration_is_local_only() {
    assert!(should_add_recent_project(&WorkspaceIdentity::Local));
    assert!(!should_add_recent_project(&WorkspaceIdentity::Remote(
        nucleotide_workspace::RemoteWorkspaceIdentity {
            kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
            name: "devbox".to_string(),
        }
    )));
}

#[test]
fn settings_file_open_backend_is_local() {
    assert!(matches!(
        settings_file_open_backend().identity(),
        WorkspaceIdentity::Local
    ));
}

#[test]
fn remote_open_failure_guidance_is_specific_for_ssh() {
    let status = remote_open_failure_status(
        Path::new("ssh://me@example.com:2222/home/me/project"),
        &anyhow::anyhow!("connection refused"),
    );

    assert_eq!(status.severity, Severity::Error);
    assert!(status.status.contains("ssh -p 2222 me@example.com"));
    assert!(status.status.contains("/home/me/project"));
    assert!(status.status.contains("remote helper install settings"));
}

#[test]
fn remote_open_failure_guidance_is_specific_for_wsl() {
    let status = remote_open_failure_status(
        Path::new(r"\\wsl.localhost\Ubuntu\home\me\project"),
        &anyhow::anyhow!("helper missing"),
    );

    assert_eq!(status.severity, Severity::Error);
    assert!(status.status.contains("WSL distro `Ubuntu`"));
    assert!(status.status.contains("/home/me/project"));
    assert!(status.status.contains("NUCLEOTIDE_REMOTE_HELPER"));
}

#[test]
fn pending_remote_open_deduplicates_only_the_same_target_and_options() {
    let target = RemoteOpenTarget {
        path: PathBuf::from("ssh://devbox/home/me/project"),
        kind: RemoteOpenTargetKind::Directory,
    };
    let backend_options = nucleotide_remote::RemoteWorkspaceBackendOptions::default();
    let pending = PendingRemoteOpen {
        id: 1,
        target: target.clone(),
        backend_options: backend_options.clone(),
        workspace_root: target.path.clone(),
        activity_id: BackgroundActivityId(1),
        startup: nucleotide_remote::RemoteStartupAttempt::new(
            nucleotide_remote::DEFAULT_REMOTE_STARTUP_TIMEOUT,
        ),
    };

    assert!(pending_remote_open_matches(
        Some(&pending),
        &target,
        &backend_options
    ));
    assert!(!pending_remote_open_matches(
        Some(&pending),
        &RemoteOpenTarget {
            path: target.path.join("README.md"),
            kind: RemoteOpenTargetKind::File,
        },
        &backend_options,
    ));
    let mut changed_options = backend_options.clone();
    changed_options.ssh_connect_timeout_secs = Some(
        backend_options
            .ssh_connect_timeout_secs
            .unwrap_or_default()
            .saturating_add(1),
    );
    assert!(!pending_remote_open_matches(
        Some(&pending),
        &target,
        &changed_options,
    ));
    assert!(!pending_remote_open_matches(
        None,
        &target,
        &backend_options
    ));

    let startup = pending.startup.context();
    drop(pending);
    assert!(nucleotide_remote::remote_startup_was_cancelled(
        &startup.check().unwrap_err()
    ));
}

#[test]
fn native_window_document_path_is_local_only() {
    let local_path = PathBuf::from("/tmp/project/src/main.rs");
    assert_eq!(
        native_window_document_path(Some(local_path.clone()), &WorkspaceIdentity::Local),
        Some(local_path)
    );

    assert_eq!(
        native_window_document_path(
            Some(PathBuf::from("ssh://devbox/home/me/project/src/main.rs")),
            &WorkspaceIdentity::Local
        ),
        None
    );
    assert_eq!(
        native_window_document_path(
            Some(PathBuf::from("/home/me/project/src/main.rs")),
            &WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
                kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
                name: "devbox".to_string(),
            })
        ),
        None
    );
}

#[test]
fn terminal_directory_for_workspace_path_keeps_local_paths() {
    let directory = Path::new("/tmp/project/src");

    assert_eq!(
        terminal_directory_for_workspace_path(directory, None, &WorkspaceIdentity::Local),
        directory
    );
}

#[test]
fn terminal_directory_for_workspace_path_rebases_ssh_native_paths_to_display_root() {
    let identity = WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
        kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let directory = Path::new("/home/me/project/src");
    let project_root = Path::new("ssh://devbox/home/me/project");

    let cwd = terminal_directory_for_workspace_path(directory, Some(project_root), &identity);

    assert_eq!(cwd, PathBuf::from("ssh://devbox/home/me/project/src"));
    assert!(classify_workspace_location(&cwd).is_remote());
}

#[test]
fn terminal_directory_for_workspace_path_rebases_wsl_native_paths_to_display_root() {
    let identity = WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
        kind: nucleotide_workspace::RemoteWorkspaceKind::Wsl,
        name: "Ubuntu".to_string(),
    });
    let directory = Path::new("/home/me/project/src");
    let project_root = Path::new("//wsl.localhost/Ubuntu/home/me/project");

    let cwd = terminal_directory_for_workspace_path(directory, Some(project_root), &identity);

    assert_eq!(
        cwd,
        PathBuf::from("//wsl.localhost/Ubuntu/home/me/project/src")
    );
    assert!(classify_workspace_location(&cwd).is_remote());
}

#[test]
fn statusbar_text_shortening_collapses_whitespace() {
    assert_eq!(
        shorten_statusbar_text("  indexing\nworkspace\tfiles  ", 64),
        "indexing workspace files"
    );
}

#[test]
fn statusbar_text_shortening_caps_display_width() {
    assert_eq!(
        shorten_statusbar_text("abcdefghijklmnopqrstuvwxyz", 10),
        "abcdefghi…"
    );
}

#[test]
fn statusbar_text_shortening_counts_characters() {
    assert_eq!(shorten_statusbar_text("éééééé", 4), "ééé…");
}

#[test]
fn statusbar_notifications_use_phosphor_icons() {
    assert_eq!(
        statusbar_notification_icon(StatusBarNotificationSeverity::Info),
        "icons/info.svg"
    );
    assert_eq!(
        statusbar_notification_icon(StatusBarNotificationSeverity::Success),
        "icons/square-check-big.svg"
    );
    assert_eq!(
        statusbar_notification_icon(StatusBarNotificationSeverity::Warning),
        "icons/triangle-alert.svg"
    );
    assert_eq!(
        statusbar_notification_icon(StatusBarNotificationSeverity::Error),
        "icons/circle-x.svg"
    );
}

#[test]
fn statusbar_document_metadata_labels_are_compact() {
    assert_eq!(statusbar_line_ending_label("\n"), "LF");
    assert_eq!(statusbar_line_ending_label("\r\n"), "CRLF");
    assert_eq!(statusbar_language_label(None), "Plain Text");
    assert_eq!(statusbar_language_label(Some("rust")), "Rust");
    assert_eq!(abbreviated_vcs_ref("0123456789abcdef"), "01234567");
}

#[test]
fn statusbar_prioritizes_document_and_branch_labels_over_secondary_metadata() {
    assert_eq!(
        statusbar_file_label(Path::new("/workspace/crates/nucleotide/src/lib.rs")),
        "lib.rs"
    );
    assert!(statusbar_show_secondary_document_metadata(
        StatusBarDensity::Wide,
        Some("main")
    ));
    assert!(!statusbar_show_secondary_document_metadata(
        StatusBarDensity::Wide,
        Some("feat/modern-ui-refresh")
    ));
    assert!(!statusbar_show_secondary_document_metadata(
        StatusBarDensity::Medium,
        Some("main")
    ));
}

#[test]
fn latest_background_activity_uses_most_recent_active_wait() {
    let activities = vec![
        BackgroundActivity {
            id: BackgroundActivityId(1),
            message: "Loading remote file".to_string(),
        },
        BackgroundActivity {
            id: BackgroundActivityId(2),
            message: "Running tests".to_string(),
        },
    ];

    let activity = latest_background_activity(&activities).expect("activity");

    assert_eq!(activity.id, BackgroundActivityId(2));
    assert_eq!(activity.message, "Running tests");
}

#[test]
fn latest_background_activity_is_none_when_idle() {
    assert!(latest_background_activity(&[]).is_none());
}

#[test]
fn statusbar_lsp_summary_prefers_working_server_over_focused_server() {
    let focused_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(1).into();
    let working_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(2).into();
    let mut state = nucleotide_lsp::LspState::new();

    state.register_server(focused_server_id, "rust-analyzer".to_string(), None);
    state.register_server(working_server_id, "pyright".to_string(), None);
    state.add_progress(nucleotide_lsp::LspProgress {
        server_id: working_server_id,
        token: "workspace-index".to_string(),
        title: "indexing".to_string(),
        message: Some("workspace".to_string()),
        percentage: None,
    });

    let summary = statusbar_lsp_summary_for_state(&state, Some(focused_server_id));

    assert_eq!(summary.server_name, "pyright");
    assert_eq!(summary.kind, LspStatusKind::Working);
    assert_eq!(summary.title, "indexing");
}

#[test]
fn statusbar_lsp_summary_uses_focused_server_when_idle() {
    let focused_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(3).into();
    let other_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(4).into();
    let mut state = nucleotide_lsp::LspState::new();

    state.register_server(focused_server_id, "rust-analyzer".to_string(), None);
    state.update_server_status(focused_server_id, nucleotide_lsp::ServerStatus::Running);
    state.register_server(other_server_id, "pyright".to_string(), None);
    state.update_server_status(other_server_id, nucleotide_lsp::ServerStatus::Running);

    let summary = statusbar_lsp_summary_for_state(&state, Some(focused_server_id));

    assert_eq!(summary.server_name, "rust-analyzer");
    assert_eq!(summary.kind, LspStatusKind::Idle);
    assert_eq!(summary.title, "Ready");
}

#[test]
fn lsp_restart_plan_uses_project_root_and_counts_only_running_planned_servers() {
    let rust_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(5).into();
    let mut state = nucleotide_lsp::LspState::new();
    state.begin_project_session(9, PathBuf::from("/workspace"));
    state.set_project_server_plan(&[
        ("rust".to_string(), "rust-analyzer".to_string()),
        ("toml".to_string(), "taplo".to_string()),
    ]);
    state.register_server(rust_server_id, "rust-analyzer".to_string(), None);

    let plan = lsp_restart_plan(&state).expect("restart plan");

    assert_eq!(plan.workspace_root, PathBuf::from("/workspace"));
    assert_eq!(plan.server_count, 1);
}

#[test]
fn statusbar_mode_offset_tracks_editor_edge() {
    let sizes = nucleotide_ui::tokens::DesignTokens::light().sizes;
    let open = StatusBarGeometry::new(1_200.0, true, 280.0, &sizes);
    let closed = StatusBarGeometry::new(1_200.0, false, 280.0, &sizes);

    assert_eq!(open.sidebar_width, px(280.0));
    assert_eq!(closed.sidebar_width, px(0.0));
}

#[test]
fn statusbar_lsp_width_is_stable_within_each_breakpoint() {
    let sizes = nucleotide_ui::tokens::DesignTokens::light().sizes;
    let wide_idle = StatusBarGeometry::new(1_200.0, true, 240.0, &sizes);
    let wide_working = StatusBarGeometry::new(1_500.0, false, 0.0, &sizes);
    let medium = StatusBarGeometry::new(900.0, true, 320.0, &sizes);
    let compact = StatusBarGeometry::new(700.0, true, 300.0, &sizes);

    assert_eq!(wide_idle.density, StatusBarDensity::Wide);
    assert_eq!(medium.density, StatusBarDensity::Medium);
    assert_eq!(compact.density, StatusBarDensity::Compact);
    assert_eq!(wide_idle.lsp_width, wide_working.lsp_width);
    assert_eq!(wide_idle.lsp_width, px(132.0));
    assert_eq!(medium.lsp_width, px(112.0));
    assert_eq!(compact.lsp_width, px(36.0));
    assert_eq!(wide_idle.position_width, wide_working.position_width);
    assert_eq!(wide_idle.utility_width, wide_working.utility_width);
}

#[test]
fn statusbar_breakpoints_follow_research_priority() {
    assert_eq!(
        StatusBarDensity::for_available_width(780.0),
        StatusBarDensity::Wide
    );
    assert_eq!(
        StatusBarDensity::for_available_width(779.0),
        StatusBarDensity::Medium
    );
    assert_eq!(
        StatusBarDensity::for_available_width(400.0),
        StatusBarDensity::Medium
    );
    assert_eq!(
        StatusBarDensity::for_available_width(399.0),
        StatusBarDensity::Compact
    );
}

#[test]
fn statusbar_density_accounts_for_resized_sidebar() {
    let sizes = nucleotide_ui::tokens::DesignTokens::light().sizes;

    let narrow_editor = StatusBarGeometry::new(1_200.0, true, 1_000.0, &sizes);
    let minimum_window = StatusBarGeometry::new(400.0, true, 240.0, &sizes);

    assert_eq!(narrow_editor.density, StatusBarDensity::Compact);
    assert_eq!(minimum_window.density, StatusBarDensity::Compact);
    assert_eq!(narrow_editor.lsp_width, sizes.statusbar_lsp_width_compact);
}

#[test]
fn app_titlebar_remains_visible_with_translucent_sidebar() {
    assert!(should_render_app_titlebar(true, false, 240.0, true));
    assert!(should_render_app_titlebar(true, true, 240.0, false));
    assert!(should_render_app_titlebar(true, true, 0.0, true));
    assert!(should_render_app_titlebar(true, true, 240.0, true));
    assert!(!should_render_app_titlebar(false, true, 240.0, true));
}

#[test]
fn file_tree_content_inset_clears_macos_traffic_lights() {
    assert_eq!(f32::from(file_tree_content_top_inset(false)), 0.0);
    assert_eq!(
        f32::from(file_tree_content_top_inset(true)),
        MACOS_TRAFFIC_LIGHT_TREE_TOP_INSET_PX
    );
}

#[test]
fn translucent_sidebar_extends_into_status_bar_only_when_visible() {
    assert!(should_extend_translucent_sidebar_into_status_bar(
        true, 240.0, true
    ));
    assert!(!should_extend_translucent_sidebar_into_status_bar(
        false, 240.0, true
    ));
    assert!(!should_extend_translucent_sidebar_into_status_bar(
        true, 0.0, true
    ));
    assert!(!should_extend_translucent_sidebar_into_status_bar(
        true, 240.0, false
    ));
}

#[test]
fn native_window_title_uses_nucleotide_app_name() {
    assert_eq!(native_window_title(Some("main.rs")), "main.rs — Nucleotide");
    assert_eq!(native_window_title(Some("")), "Nucleotide");
    assert_eq!(native_window_title(None), "Nucleotide");
}

#[test]
fn configured_theme_name_uses_explicit_theme_modes() {
    let theme = crate::config::ThemeConfig {
        mode: crate::config::ThemeMode::Light,
        light_theme: Some("light-test".to_string()),
        dark_theme: Some("dark-test".to_string()),
    };
    assert_eq!(
        configured_theme_name_for_appearance(&theme, nucleotide_appearance::SystemAppearance::Dark),
        "light-test"
    );

    let theme = crate::config::ThemeConfig {
        mode: crate::config::ThemeMode::Dark,
        light_theme: Some("light-test".to_string()),
        dark_theme: Some("dark-test".to_string()),
    };
    assert_eq!(
        configured_theme_name_for_appearance(
            &theme,
            nucleotide_appearance::SystemAppearance::Light
        ),
        "dark-test"
    );
}

#[test]
fn configured_theme_name_follows_system_appearance_in_system_mode() {
    let theme = crate::config::ThemeConfig {
        mode: crate::config::ThemeMode::System,
        light_theme: Some("light-test".to_string()),
        dark_theme: Some("dark-test".to_string()),
    };

    assert_eq!(
        configured_theme_name_for_appearance(
            &theme,
            nucleotide_appearance::SystemAppearance::Light
        ),
        "light-test"
    );
    assert_eq!(
        configured_theme_name_for_appearance(&theme, nucleotide_appearance::SystemAppearance::Dark),
        "dark-test"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_recent_project_path_accepts_directories_only() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("file.txt");
    std::fs::write(&file_path, "not a project root").unwrap();

    let recent_path = windows_recent_project_path(temp_dir.path()).unwrap();
    assert!(recent_path.is_absolute());
    assert!(recent_path.is_dir());
    assert_eq!(windows_recent_project_path(&file_path), None);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_wide_nul_path_is_nul_terminated() {
    let wide_path = windows_wide_nul_path(Path::new(r"C:\Users\Example Project"));

    assert_eq!(wide_path.last().copied(), Some(0));
    assert_eq!(wide_path.iter().filter(|&&ch| ch == 0).count(), 1);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_jump_list_recent_projects_are_most_recent_first() {
    let mut recent = VecDeque::new();
    let project_a = PathBuf::from(r"C:\Users\Example\project-a");
    let project_b = PathBuf::from(r"C:\Users\Example\project-b");

    windows_update_recent_project_list(&mut recent, project_a.clone());
    windows_update_recent_project_list(&mut recent, project_b.clone());
    windows_update_recent_project_list(&mut recent, project_a.clone());

    let entries = windows_jump_list_entries(&recent);
    let expected: Vec<SmallVec<[PathBuf; 2]>> = vec![smallvec![project_a], smallvec![project_b]];

    assert_eq!(entries, expected);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_jump_list_recent_projects_are_capped() {
    let mut recent = VecDeque::new();

    for index in 0..(WINDOWS_JUMP_LIST_PROJECT_LIMIT + 2) {
        windows_update_recent_project_list(
            &mut recent,
            PathBuf::from(format!(r"C:\Users\Example\project-{index}")),
        );
    }

    assert_eq!(recent.len(), WINDOWS_JUMP_LIST_PROJECT_LIMIT);
    assert_eq!(
        recent.front(),
        Some(&PathBuf::from(format!(
            r"C:\Users\Example\project-{}",
            WINDOWS_JUMP_LIST_PROJECT_LIMIT + 1
        )))
    );
    assert_eq!(
        recent.back(),
        Some(&PathBuf::from(r"C:\Users\Example\project-2"))
    );
}

#[test]
fn completion_refinement_follows_focused_document() {
    let doc_id = DocumentId::default();

    assert!(should_refine_completion_for_focused_document(
        true,
        Some(doc_id),
        doc_id
    ));
}

#[test]
fn completion_refinement_ignores_missing_completion_or_focus() {
    let doc_id = DocumentId::default();

    assert!(!should_refine_completion_for_focused_document(
        false,
        Some(doc_id),
        doc_id
    ));
    assert!(!should_refine_completion_for_focused_document(
        true, None, doc_id
    ));
}

#[test]
fn incomplete_completion_retrigger_follows_focused_session() {
    let doc_id = DocumentId::default();
    let view_id = test_view_id(1);
    let session = ActiveCompletionSession {
        doc_id,
        view_id,
        document_version: 0,
        document_text: Rope::from_str("pri"),
        prefix_range: 0..3,
        is_incomplete: true,
        incomplete_server_ids: vec![1],
        retained_items: Vec::new(),
        requested_prefix: "pri".to_string(),
    };

    assert!(should_retrigger_incomplete_completion_for_focused_session(
        &session,
        "prin",
        Some(doc_id),
        view_id
    ));
}

#[test]
fn incomplete_completion_retrigger_ignores_complete_or_unchanged_sessions() {
    let doc_id = DocumentId::default();
    let view_id = test_view_id(1);
    let mut session = ActiveCompletionSession {
        doc_id,
        view_id,
        document_version: 0,
        document_text: Rope::from_str("pri"),
        prefix_range: 0..3,
        is_incomplete: false,
        incomplete_server_ids: vec![1],
        retained_items: Vec::new(),
        requested_prefix: "pri".to_string(),
    };

    assert!(!should_retrigger_incomplete_completion_for_focused_session(
        &session,
        "prin",
        Some(doc_id),
        view_id
    ));

    session.is_incomplete = true;
    assert!(!should_retrigger_incomplete_completion_for_focused_session(
        &session,
        "pri",
        Some(doc_id),
        view_id
    ));
    assert!(!should_retrigger_incomplete_completion_for_focused_session(
        &session, "prin", None, view_id
    ));
    assert!(!should_retrigger_incomplete_completion_for_focused_session(
        &session,
        "prin",
        Some(doc_id),
        test_view_id(2)
    ));
}

#[test]
fn incomplete_completion_retains_completed_provider_items() {
    let completed = nucleotide_events::completion::CompletionItem::new(
        "clone".to_string(),
        nucleotide_events::completion::CompletionItemKind::Method,
    )
    .with_server_id(Some(1));
    let incomplete = nucleotide_events::completion::CompletionItem::new(
        "fmt".to_string(),
        nucleotide_events::completion::CompletionItemKind::Method,
    )
    .with_server_id(Some(2));
    let local = nucleotide_events::completion::CompletionItem::new(
        "local".to_string(),
        nucleotide_events::completion::CompletionItemKind::Text,
    );

    let retained = retained_completion_items_for_completed_providers(
        &[completed.clone(), incomplete, local],
        &[2],
    );

    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].label, completed.label);
    assert_eq!(retained[0].server_id, Some(1));
}

#[test]
fn completion_locality_key_uses_filter_text_first() {
    let item = nucleotide_ui::completion_v2::CompletionItem::new("fmt(${1:f})")
        .with_display_text("fmt(...)")
        .with_filter_text("Debug::fmt");

    assert_eq!(completion_locality_key(&item).as_deref(), Some("debug"));
}

#[test]
fn completion_locality_score_prefers_nearby_lines() {
    let text = "clone\n\nfmt\n\ninto\n";

    assert!(
        completion_locality_score_for_text(text, 2, "fmt")
            > completion_locality_score_for_text(text, 2, "clone")
    );
    assert_eq!(completion_locality_score_for_text(text, 2, "missing"), 0);
}

#[test]
fn completion_memory_prioritizes_recent_prefix_match() {
    let mut memory = CompletionMemory::default();
    let old_key = CompletionMemoryKey {
        language: "rust".to_string(),
        prefix: "fo".to_string(),
        kind: Some(nucleotide_ui::completion_v2::CompletionItemKind::Function),
        insert_text: "foo".to_string(),
    };
    let recent_key = CompletionMemoryKey {
        language: "rust".to_string(),
        prefix: "fo".to_string(),
        kind: Some(nucleotide_ui::completion_v2::CompletionItemKind::Function),
        insert_text: "foobar".to_string(),
    };

    memory.memorize(old_key.clone());
    memory.memorize(recent_key.clone());

    assert!(memory.priority(&recent_key) > memory.priority(&old_key));
    assert_eq!(
        memory.priority(&CompletionMemoryKey {
            language: "rust".to_string(),
            prefix: "ba".to_string(),
            kind: Some(nucleotide_ui::completion_v2::CompletionItemKind::Function),
            insert_text: "foobar".to_string(),
        }),
        0
    );
}

#[test]
fn completion_commit_character_uses_unmodified_printable_key() {
    assert_eq!(
        completion_commit_character_from_key("(", Some("("), false),
        Some('(')
    );
    assert_eq!(
        completion_commit_character_from_key("9", Some("("), false),
        Some('(')
    );
    assert_eq!(
        completion_commit_character_from_key("(", Some("("), true),
        None
    );
    assert_eq!(
        completion_commit_character_from_key("enter", None, false),
        None
    );
}

#[test]
fn completion_word_start_uses_character_offsets() {
    let rope = Rope::from("héllo world");
    let cursor = 5;

    assert_eq!(completion_word_start(rope.slice(..), cursor), 0);
}

#[test]
fn completion_edit_offset_converts_lsp_range() {
    let rope = Rope::from("let value = old;");
    let edit = nucleotide_ui::CompletionTextEdit {
        range: nucleotide_ui::CompletionRange {
            start: nucleotide_ui::CompletionPosition {
                line: 0,
                character: 12,
            },
            end: nucleotide_ui::CompletionPosition {
                line: 0,
                character: 15,
            },
        },
        new_text: "new".to_string(),
    };

    let (offset, start) = completion_edit_offset(&rope, &edit, OffsetEncoding::Utf8, 15).unwrap();

    assert_eq!(offset, (-3, 0));
    assert_eq!(start, 12);
}

#[test]
fn completion_edit_offset_extends_stale_range_to_live_word() {
    let rope = Rope::from("println");
    let edit = nucleotide_ui::CompletionTextEdit {
        range: nucleotide_ui::CompletionRange {
            start: nucleotide_ui::CompletionPosition {
                line: 0,
                character: 0,
            },
            end: nucleotide_ui::CompletionPosition {
                line: 0,
                character: 5,
            },
        },
        new_text: "println!()".to_string(),
    };

    let (offset, start) = completion_edit_offset(&rope, &edit, OffsetEncoding::Utf8, 7).unwrap();

    assert_eq!(offset, (-7, 0));
    assert_eq!(start, 0);
}

#[test]
fn completion_edit_offset_keeps_range_when_it_starts_inside_word() {
    let rope = Rope::from("println");
    let edit = nucleotide_ui::CompletionTextEdit {
        range: nucleotide_ui::CompletionRange {
            start: nucleotide_ui::CompletionPosition {
                line: 0,
                character: 2,
            },
            end: nucleotide_ui::CompletionPosition {
                line: 0,
                character: 5,
            },
        },
        new_text: "intln!()".to_string(),
    };

    let (offset, start) = completion_edit_offset(&rope, &edit, OffsetEncoding::Utf8, 7).unwrap();

    assert_eq!(offset, (-5, -2));
    assert_eq!(start, 2);
}

fn test_snippet_render_ctx() -> helix_core::snippets::SnippetRenderCtx {
    helix_core::snippets::SnippetRenderCtx {
        resolve_var: Box::new(|_| None),
        tab_width: 4,
        indent_style: helix_core::indent::IndentStyle::Spaces(4),
        line_ending: "\n",
    }
}

#[test]
fn snippet_completion_transaction_preserves_active_placeholder() {
    let mut rope = Rope::from("pri");
    let selection = Selection::point(3);
    let mut snippet_ctx = test_snippet_render_ctx();

    let (transaction, rendered_snippet) = snippet_completion_transaction(
        &rope,
        &selection,
        "println(${1:value});$0",
        None,
        false,
        &mut snippet_ctx,
    )
    .unwrap();

    assert!(transaction.apply(&mut rope));
    assert_eq!(rope.to_string(), "println(value);");
    assert!(helix_core::snippets::ActiveSnippet::new(rendered_snippet).is_some());

    let primary = transaction.selection().unwrap().primary();
    assert_eq!(primary.from(), 8);
    assert_eq!(primary.to(), 13);
}

#[test]
fn snippet_completion_transaction_uses_lsp_edit_range() {
    let mut rope = Rope::from("let value = old;");
    let selection = Selection::point(15);
    let mut snippet_ctx = test_snippet_render_ctx();
    let edit_offset = Some((-3, 0));

    let (transaction, rendered_snippet) = snippet_completion_transaction(
        &rope,
        &selection,
        "${1:new_value}$0",
        edit_offset,
        false,
        &mut snippet_ctx,
    )
    .unwrap();

    assert!(transaction.apply(&mut rope));
    assert_eq!(rope.to_string(), "let value = new_value;");
    assert!(helix_core::snippets::ActiveSnippet::new(rendered_snippet).is_some());

    let primary = transaction.selection().unwrap().primary();
    assert_eq!(primary.from(), 12);
    assert_eq!(primary.to(), 21);
}

fn test_view_id(index: u64) -> ViewId {
    ViewId::from(KeyData::from_ffi((1_u64 << 32) | index))
}

#[test]
fn code_action_enabled_filters_disabled_actions_like_helix() {
    let enabled_action = test_code_action("enabled", None, false, false, false);
    let disabled_action = test_code_action("disabled", None, false, false, true);
    let command = lsp::CodeActionOrCommand::Command(lsp::Command {
        title: "command".to_string(),
        command: "server.command".to_string(),
        arguments: None,
    });

    assert!(code_action_enabled(&enabled_action));
    assert!(code_action_enabled(&command));
    assert!(!code_action_enabled(&disabled_action));
}

#[test]
fn code_action_metadata_label_includes_available_lsp_metadata() {
    let mut action = match test_code_action(
        "quick fix",
        Some(lsp::CodeActionKind::QUICKFIX),
        true,
        true,
        false,
    ) {
        lsp::CodeActionOrCommand::CodeAction(action) => action,
        lsp::CodeActionOrCommand::Command(_) => unreachable!(),
    };
    action.data = Some(serde_json::json!({ "token": "lazy" }));

    let label = code_action_metadata_label(
        &lsp::CodeActionOrCommand::CodeAction(action),
        "rust-analyzer",
    );

    assert_eq!(
        label,
        "quickfix · preferred · fixes diagnostics · resolves on apply · rust-analyzer"
    );

    let command = lsp::CodeActionOrCommand::Command(lsp::Command {
        title: "Run command".to_string(),
        command: "server.command".to_string(),
        arguments: None,
    });

    assert_eq!(
        code_action_metadata_label(&command, "test-ls"),
        "command: server.command · test-ls"
    );
}

#[test]
fn code_actions_sort_like_helix_by_category_and_relevance() {
    let mut actions = vec![
        test_code_action(
            "source preferred diagnostic",
            Some(lsp::CodeActionKind::SOURCE),
            true,
            true,
            false,
        ),
        test_code_action(
            "quickfix preferred no diagnostic",
            Some(lsp::CodeActionKind::QUICKFIX),
            false,
            true,
            false,
        ),
        test_code_action(
            "quickfix diagnostic",
            Some(lsp::CodeActionKind::QUICKFIX),
            true,
            false,
            false,
        ),
        test_code_action(
            "refactor extract preferred diagnostic",
            Some(lsp::CodeActionKind::REFACTOR_EXTRACT),
            true,
            true,
            false,
        ),
        test_code_action(
            "quickfix preferred diagnostic",
            Some(lsp::CodeActionKind::QUICKFIX),
            true,
            true,
            false,
        ),
    ];

    sort_code_actions_like_helix(&mut actions);

    let titles = actions.iter().map(code_action_title).collect::<Vec<_>>();

    assert_eq!(
        titles,
        vec![
            "quickfix preferred diagnostic",
            "quickfix diagnostic",
            "quickfix preferred no diagnostic",
            "refactor extract preferred diagnostic",
            "source preferred diagnostic",
        ]
    );
}

#[test]
fn tab_bar_layout_height_matches_rendered_row_count() {
    let row_height = px(32.0);

    assert_eq!(
        tab_bar_layout_height(row_height, false, true, true),
        row_height
    );
    assert_eq!(
        tab_bar_layout_height(row_height, true, true, true),
        px(64.0)
    );
    assert_eq!(
        tab_bar_layout_height(row_height, true, true, false),
        row_height
    );
    assert_eq!(
        tab_bar_layout_height(row_height, true, false, true),
        row_height
    );
}

#[test]
fn tab_bar_height_for_editor_matches_bufferline_visibility() {
    use helix_view::editor::BufferLine;

    let row_height = px(32.0);

    assert_eq!(
        tab_bar_height_for_editor(true, &BufferLine::Never, 3, row_height, true, true, true),
        px(0.0)
    );
    assert_eq!(
        tab_bar_height_for_editor(true, &BufferLine::Always, 1, row_height, true, true, true),
        px(64.0)
    );
    assert_eq!(
        tab_bar_height_for_editor(true, &BufferLine::Multiple, 1, row_height, true, true, true),
        px(0.0)
    );
    assert_eq!(
        tab_bar_height_for_editor(true, &BufferLine::Multiple, 2, row_height, true, true, true),
        px(64.0)
    );
    assert_eq!(
        tab_bar_height_for_editor(false, &BufferLine::Always, 2, row_height, true, true, true),
        px(0.0)
    );
}

#[test]
fn tab_context_menu_entries_match_zed_grouping() {
    let labels = Workspace::tab_context_menu_entries(false, false, false)
        .iter()
        .map(|entry| match entry {
            TabContextMenuEntry::Action(intent) => intent.label(false, false),
            TabContextMenuEntry::Separator => "|",
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "Close",
            "Close Others",
            "|",
            "Close Left",
            "Close Right",
            "|",
            "Close Clean",
            "Close All",
            "|",
            "Pin Tab"
        ]
    );

    let pinned_label = Workspace::tab_context_menu_entries(false, false, false)
        .iter()
        .rev()
        .find_map(|entry| match entry {
            TabContextMenuEntry::Action(intent) => Some(intent.label(true, false)),
            TabContextMenuEntry::Separator => None,
        });
    assert_eq!(pinned_label, Some("Unpin Tab"));
}

#[test]
fn tab_context_menu_entries_add_zed_file_path_actions_for_file_tabs() {
    let reveal_label = reveal_in_file_manager_label(false);
    let labels = Workspace::tab_context_menu_entries(true, false, false)
        .iter()
        .map(|entry| match entry {
            TabContextMenuEntry::Action(intent) => intent.label(false, false),
            TabContextMenuEntry::Separator => "|",
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "Close",
            "Close Others",
            "|",
            "Close Left",
            "Close Right",
            "|",
            "Close Clean",
            "Close All",
            "|",
            "Make File Read-Only",
            "|",
            "Copy Path",
            "Copy Relative Path",
            "|",
            reveal_label,
            "|",
            "Pin Tab"
        ]
    );
}

#[test]
fn tab_context_menu_readonly_toggle_label_matches_zed_state() {
    assert_eq!(
        TabContextMenuIntent::ToggleReadOnly.label(false, false),
        "Make File Read-Only"
    );
    assert_eq!(
        TabContextMenuIntent::ToggleReadOnly.label(false, true),
        "Make File Editable"
    );
}

#[test]
fn tab_context_menu_entries_add_reveal_project_panel_for_visible_project_paths() {
    let reveal_label = reveal_in_file_manager_label(false);
    let labels = Workspace::tab_context_menu_entries(true, true, false)
        .iter()
        .map(|entry| match entry {
            TabContextMenuEntry::Action(intent) => intent.label(false, false),
            TabContextMenuEntry::Separator => "|",
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "Close",
            "Close Others",
            "|",
            "Close Left",
            "Close Right",
            "|",
            "Close Clean",
            "Close All",
            "|",
            "Make File Read-Only",
            "|",
            "Copy Path",
            "Copy Relative Path",
            "|",
            reveal_label,
            "|",
            "Pin Tab",
            "Reveal In Project Panel"
        ]
    );
}

#[test]
fn tab_context_menu_entries_add_open_terminal_when_parent_directory_exists() {
    let reveal_label = reveal_in_file_manager_label(false);
    let labels = Workspace::tab_context_menu_entries(true, true, true)
        .iter()
        .map(|entry| match entry {
            TabContextMenuEntry::Action(intent) => intent.label(false, false),
            TabContextMenuEntry::Separator => "|",
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "Close",
            "Close Others",
            "|",
            "Close Left",
            "Close Right",
            "|",
            "Close Clean",
            "Close All",
            "|",
            "Make File Read-Only",
            "|",
            "Copy Path",
            "Copy Relative Path",
            "|",
            reveal_label,
            "|",
            "Pin Tab",
            "Reveal In Project Panel",
            "Open in Terminal"
        ]
    );
}

#[test]
fn reveal_in_file_manager_label_matches_zed_platform_label() {
    if cfg!(target_os = "macos") {
        assert_eq!(reveal_in_file_manager_label(false), "Reveal in Finder");
    } else if cfg!(target_os = "windows") {
        assert_eq!(
            reveal_in_file_manager_label(false),
            "Reveal in File Explorer"
        );
    } else {
        assert_eq!(
            reveal_in_file_manager_label(false),
            "Reveal in File Manager"
        );
    }
    assert_eq!(reveal_in_file_manager_label(true), "Reveal in File Manager");
    assert_eq!(
        ProjectTreeContextMenuIntent::RevealInOs.label(),
        TabContextMenuIntent::RevealInOs.label(false, false)
    );
}

#[test]
fn tab_context_menu_disabled_states_match_zed_rules() {
    assert!(!Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::Close,
        Some(0),
        1,
        false,
    ));
    assert!(Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::CloseOthers,
        Some(0),
        1,
        true,
    ));
    assert!(!Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::CloseOthers,
        Some(0),
        2,
        true,
    ));
    assert!(Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::CloseLeft,
        Some(0),
        3,
        true,
    ));
    assert!(Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::CloseRight,
        Some(2),
        3,
        true,
    ));
    assert!(Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::CloseClean,
        Some(1),
        3,
        false,
    ));
    assert!(!Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::CloseClean,
        Some(1),
        3,
        true,
    ));
    assert!(Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::ToggleReadOnly,
        None,
        3,
        true,
    ));
    assert!(!Workspace::tab_context_menu_intent_disabled(
        TabContextMenuIntent::ToggleReadOnly,
        Some(1),
        3,
        true,
    ));
}

#[test]
fn tab_bar_split_menu_items_match_zed_directional_split_actions() {
    let labels = Workspace::tab_bar_split_menu_intents()
        .iter()
        .map(|intent| intent.label())
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec!["Split Right", "Split Left", "Split Up", "Split Down"]
    );
}

#[test]
fn tab_bar_split_menu_commands_match_directional_helix_primitives() {
    let commands = Workspace::tab_bar_split_menu_intents()
        .iter()
        .map(|intent| intent.commands())
        .collect::<Vec<_>>();

    assert_eq!(
        commands,
        vec![
            &["vsplit"][..],
            &["vsplit", "swap_view_left"][..],
            &["hsplit", "swap_view_up"][..],
            &["hsplit"][..],
        ]
    );
}

#[test]
fn terminal_spawn_cwd_uses_loaded_project_root() {
    let project_root = PathBuf::from("/tmp/example-project");

    assert_eq!(
        Workspace::terminal_spawn_cwd(Some(project_root.as_path())),
        Some(project_root)
    );
    assert_eq!(Workspace::terminal_spawn_cwd(None), None);
}

#[test]
fn remote_terminal_proxy_command_routes_wsl_cwd_through_helper() {
    let args = vec!["test".to_string()];
    let workspace_location =
        classify_workspace_location(Path::new(r"\\wsl.localhost\Ubuntu\home\me\project"));
    let cwd_location = classify_workspace_location(Path::new(
        r"\\wsl.localhost\Ubuntu\home\me\project\crates\nucleotide",
    ));
    let options = nucleotide_remote::RemoteWorkspaceBackendOptions::default();
    let spec = Workspace::remote_terminal_proxy_command_with_options(
        &workspace_location,
        &cwd_location,
        &options,
        None,
        Some(("cargo", &args)),
        &[],
    )
    .unwrap();

    assert_eq!(spec.program, std::ffi::OsString::from("wsl.exe"));
    assert!(
        spec.args
            .contains(&std::ffi::OsString::from("terminal-proxy"))
    );
    assert!(spec.args.contains(&std::ffi::OsString::from("cargo")));
    assert!(spec.args.contains(&std::ffi::OsString::from("test")));
    assert!(spec.args.windows(2).any(|args| {
        args == [
            std::ffi::OsString::from("--workspace"),
            std::ffi::OsString::from("/home/me/project"),
        ]
    }));
    assert!(spec.args.windows(2).any(|args| {
        args == [
            std::ffi::OsString::from("--cwd"),
            std::ffi::OsString::from("/home/me/project/crates/nucleotide"),
        ]
    }));
}

#[test]
fn remote_terminal_proxy_command_ignores_local_cwd() {
    let location = classify_workspace_location(Path::new("/tmp/project"));
    let options = nucleotide_remote::RemoteWorkspaceBackendOptions::default();
    let spec = Workspace::remote_terminal_proxy_command_with_options(
        &location,
        &location,
        &options,
        None,
        None,
        &[],
    );

    assert!(spec.is_none());
}

#[test]
fn remote_terminal_proxy_command_rejects_cwd_outside_workspace() {
    let workspace_location =
        classify_workspace_location(Path::new(r"\\wsl.localhost\Ubuntu\home\me\project"));
    let cwd_location =
        classify_workspace_location(Path::new(r"\\wsl.localhost\Ubuntu\home\me\outside"));
    let options = nucleotide_remote::RemoteWorkspaceBackendOptions::default();

    let spec = Workspace::remote_terminal_proxy_command_with_options(
        &workspace_location,
        &cwd_location,
        &options,
        None,
        Some(("cargo", &[])),
        &[],
    );

    assert!(spec.is_none());
}

#[test]
fn remote_terminal_proxy_command_rejects_different_remote_target() {
    let workspace_location =
        classify_workspace_location(Path::new(r"\\wsl.localhost\Ubuntu\home\me\project"));
    let cwd_location =
        classify_workspace_location(Path::new(r"\\wsl.localhost\Debian\home\me\project"));
    let options = nucleotide_remote::RemoteWorkspaceBackendOptions::default();

    let spec = Workspace::remote_terminal_proxy_command_with_options(
        &workspace_location,
        &cwd_location,
        &options,
        None,
        Some(("cargo", &[])),
        &[],
    );

    assert!(spec.is_none());
}

#[test]
fn terminal_cwd_matching_detects_project_root_changes() {
    let old_root = PathBuf::from("/tmp/old-project");
    let new_root = PathBuf::from("/tmp/new-project");

    assert!(Workspace::terminal_cwd_matches(
        Some(old_root.as_path()),
        Some(old_root.as_path())
    ));
    assert!(!Workspace::terminal_cwd_matches(
        Some(old_root.as_path()),
        Some(new_root.as_path())
    ));
    assert!(!Workspace::terminal_cwd_matches(
        None,
        Some(new_root.as_path())
    ));
}

#[test]
fn tab_bar_new_menu_entries_match_zed_grouping() {
    let labels = Workspace::tab_bar_new_menu_entries()
        .iter()
        .map(|entry| match entry {
            TabBarNewMenuEntry::Action(intent) => intent.label(),
            TabBarNewMenuEntry::Separator => "|",
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "New File",
            "Open File",
            "|",
            "Search Project",
            "Search Symbols",
            "|",
            "New Terminal",
            "New Center Terminal"
        ]
    );
}

#[test]
fn file_tree_width_clamps_to_available_space() {
    assert_eq!(
        Workspace::max_file_tree_width(1000.0),
        1000.0 - FILE_TREE_MIN_EDITOR_WIDTH
    );
    assert_eq!(Workspace::max_file_tree_width(200.0), FILE_TREE_MIN_WIDTH);
    assert_eq!(
        Workspace::clamped_file_tree_sidebar_width(10.0, 1000.0),
        FILE_TREE_MIN_WIDTH
    );
    assert_eq!(
        Workspace::clamped_file_tree_sidebar_width(1200.0, 1000.0),
        1000.0 - FILE_TREE_MIN_EDITOR_WIDTH
    );
}

#[test]
fn documentation_sidebar_width_clamps_to_available_space() {
    assert_eq!(
        Workspace::max_documentation_sidebar_width(1000.0),
        DOC_SIDEBAR_MAX_WIDTH
    );
    assert_eq!(Workspace::max_documentation_sidebar_width(500.0), 260.0);
    assert_eq!(
        Workspace::clamped_documentation_sidebar_width(100.0, 1000.0),
        DOC_SIDEBAR_MIN_WIDTH
    );
    assert_eq!(
        Workspace::clamped_documentation_sidebar_width(700.0, 1000.0),
        DOC_SIDEBAR_MAX_WIDTH
    );
    assert_eq!(
        Workspace::clamped_documentation_sidebar_width(360.0, 500.0),
        260.0
    );
}

#[test]
fn file_tree_default_width_uses_default_and_viewport_limit() {
    assert_eq!(
        Workspace::clamped_file_tree_default_width(1000.0),
        FILE_TREE_DEFAULT_WIDTH
    );
    assert_eq!(Workspace::clamped_file_tree_default_width(360.0), 160.0);
    assert_eq!(
        Workspace::clamped_file_tree_default_width(250.0),
        FILE_TREE_MIN_WIDTH
    );
}

#[test]
fn file_tree_config_from_gui_uses_configured_file_tree_options() {
    let mut gui_config = crate::config::GuiConfig::default();
    gui_config.file_tree.density = crate::file_tree::FileTreeDisplayDensity::Relaxed;
    gui_config.file_tree.flatten_empty_directories = false;
    gui_config.ui.look = crate::config::UiLook::System;

    let file_tree_config = file_tree_config_from_gui(&gui_config);

    assert_eq!(
        file_tree_config.density,
        crate::file_tree::FileTreeDisplayDensity::Relaxed
    );
    assert!(!file_tree_config.flatten_empty_directories);
    assert_eq!(
        file_tree_config.translucent_background,
        cfg!(target_os = "macos")
    );
}

#[test]
fn active_unpinned_tab_scroll_index_ignores_pinned_tabs() {
    let items = ['a', 'b', 'c', 'd', 'e'];
    let pinned = HashSet::from(['b', 'd']);

    assert_eq!(
        active_unpinned_tab_scroll_index(&items, &pinned, 'a'),
        Some(0)
    );
    assert_eq!(
        active_unpinned_tab_scroll_index(&items, &pinned, 'c'),
        Some(1)
    );
    assert_eq!(
        active_unpinned_tab_scroll_index(&items, &pinned, 'e'),
        Some(2)
    );
}

#[test]
fn active_unpinned_tab_scroll_index_skips_pinned_and_missing_tabs() {
    let items = ['a', 'b', 'c', 'd'];
    let pinned = HashSet::from(['a', 'b']);

    assert_eq!(active_unpinned_tab_scroll_index(&items, &pinned, 'a'), None);
    assert_eq!(active_unpinned_tab_scroll_index(&items, &pinned, 'b'), None);
    assert_eq!(active_unpinned_tab_scroll_index(&items, &pinned, 'x'), None);
}

#[test]
fn active_tab_auto_scroll_respects_zed_manual_scroll_suppression() {
    assert!(should_scroll_active_tab(false, None, Some('a')));
    assert!(should_scroll_active_tab(false, Some('a'), Some('b')));
    assert!(!should_scroll_active_tab(false, Some('a'), Some('a')));
    assert!(!should_scroll_active_tab(false, Some('a'), None));
    assert!(!should_scroll_active_tab(true, Some('a'), Some('b')));
}

#[test]
fn unpin_all_tabs_reports_no_ops_when_nothing_is_pinned() {
    let mut pinned = HashSet::<char>::new();

    assert!(!unpin_all_tabs(&mut pinned));
    assert!(pinned.is_empty());
}

#[test]
fn unpin_all_tabs_preserves_current_tab_order() {
    let items = vec!['a', 'b', 'c'];
    let mut pinned = HashSet::from(['a', 'b']);

    assert!(unpin_all_tabs(&mut pinned));
    assert!(pinned.is_empty());
    assert_eq!(zed_style_tab_order(&items, &pinned), items);
}

#[test]
fn preview_tab_toggle_plan_unpreviews_active_preview_tab() {
    let previews = HashSet::from(['b']);

    assert_eq!(
        preview_tab_toggle_plan(&previews, &'b'),
        PreviewTabTogglePlan::Unpreview
    );
}

#[test]
fn preview_tab_toggle_plan_marks_active_non_preview_tab_as_preview() {
    let previews = HashSet::from(['b', 'd']);

    assert_eq!(
        preview_tab_toggle_plan(&previews, &'c'),
        PreviewTabTogglePlan::Preview
    );
}

#[test]
fn project_panel_preview_requires_global_and_project_panel_settings() {
    assert!(should_create_project_panel_preview_tab(true, true, false));
    assert!(!should_create_project_panel_preview_tab(false, true, false));
    assert!(!should_create_project_panel_preview_tab(true, false, false));
}

#[test]
fn project_panel_preview_does_not_reclassify_existing_tabs() {
    assert!(!should_create_project_panel_preview_tab(true, true, true));
}

#[test]
fn changed_preview_documents_are_unpreviewed_only_after_edits() {
    assert!(should_unpreview_changed_document(true, true));
    assert!(!should_unpreview_changed_document(true, false));
    assert!(!should_unpreview_changed_document(false, true));
    assert!(!should_unpreview_changed_document(false, false));
}

#[test]
fn close_others_unpreviews_retained_preview_tab() {
    assert!(should_unpreview_retained_tab_after_close_others(true));
    assert!(!should_unpreview_retained_tab_after_close_others(false));
}

#[test]
fn unsaved_buffers_status_matches_helix_close_error_shape() {
    let status =
        unsaved_buffers_remaining_status(vec!["main.rs".to_string(), "lib.rs".to_string()]);

    assert_eq!(
        status.status,
        "2 unsaved buffers remaining: [\"main.rs\", \"lib.rs\"]"
    );
    assert_eq!(status.severity, Severity::Error);
}

#[test]
fn unsaved_close_confirmation_copy_names_single_and_batch() {
    assert_eq!(unsaved_close_confirmation_title(1), "Close Unsaved Buffer");
    assert_eq!(
        unsaved_close_confirmation_message(&["main.rs".to_string()]),
        "'main.rs' has unsaved changes. Close without saving?"
    );

    assert_eq!(unsaved_close_confirmation_title(2), "Close Unsaved Buffers");
    assert_eq!(
        unsaved_close_confirmation_message(&["main.rs".to_string(), "lib.rs".to_string()]),
        "2 buffers have unsaved changes: main.rs, lib.rs. Close without saving?"
    );
}

#[test]
fn active_tab_close_plan_closes_unpinned_active_tab() {
    let items = ['a', 'b', 'c'];
    let pinned = HashSet::from(['a']);

    assert_eq!(
        active_tab_close_plan(&items, &pinned, Some('b')),
        ActiveTabClosePlan::Close('b')
    );
}

#[test]
fn active_tab_close_plan_activates_unpinned_tab_instead_of_closing_pinned_active_tab() {
    let items = ['a', 'b', 'c'];
    let pinned = HashSet::from(['a', 'b']);

    assert_eq!(
        active_tab_close_plan(&items, &pinned, Some('a')),
        ActiveTabClosePlan::Activate('c')
    );
}

#[test]
fn active_tab_close_plan_ignores_pinned_active_tab_when_no_unpinned_tab_exists() {
    let items = ['a', 'b'];
    let pinned = HashSet::from(['a', 'b']);

    assert_eq!(
        active_tab_close_plan(&items, &pinned, Some('a')),
        ActiveTabClosePlan::Ignore
    );
}

#[test]
fn active_tab_close_plan_ignores_missing_active_tab() {
    let items = ['a', 'b'];
    let pinned = HashSet::from(['a']);

    assert_eq!(
        active_tab_close_plan(&items, &pinned, None),
        ActiveTabClosePlan::Ignore
    );
    assert_eq!(
        active_tab_close_plan(&items, &pinned, Some('x')),
        ActiveTabClosePlan::Ignore
    );
}

#[test]
fn tab_double_click_renames_file_tabs_and_activates_pathless_tabs() {
    assert_eq!(tab_double_click_plan(true), TabDoubleClickPlan::Rename);
    assert_eq!(tab_double_click_plan(false), TabDoubleClickPlan::Activate);
}

#[test]
fn deleted_document_path_matches_zed_backing_file_rule() {
    assert!(!is_deleted_document_path(None, &WorkspaceIdentity::Local));

    let dir = tempfile::tempdir().unwrap();
    let existing_path = dir.path().join("existing.rs");
    std::fs::write(&existing_path, "").unwrap();
    assert!(!is_deleted_document_path(
        Some(existing_path.as_path()),
        &WorkspaceIdentity::Local
    ));

    let missing_path = dir.path().join("missing.rs");
    assert!(is_deleted_document_path(
        Some(missing_path.as_path()),
        &WorkspaceIdentity::Local
    ));
}

#[test]
fn deleted_document_path_does_not_probe_remote_paths() {
    let remote_identity =
        WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
            kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
            name: "devbox".to_string(),
        });

    assert!(!is_deleted_document_path(
        Some(Path::new("/remote/project/missing.rs")),
        &remote_identity
    ));
}

#[test]
fn open_directory_root_uses_remote_directory_without_host_root_search() {
    let remote_identity =
        WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
            kind: nucleotide_workspace::RemoteWorkspaceKind::Wsl,
            name: "Ubuntu".to_string(),
        });
    let remote_dir = Path::new("//wsl.localhost/Ubuntu/home/me/project/crates/app");

    let root = workspace_root_for_open_directory(remote_dir, &remote_identity);

    assert_eq!(root, remote_dir);
}

fn activation_doc(id: char, age: u64) -> TabActivationDocument<char> {
    TabActivationDocument {
        id,
        focused_at: std::time::Instant::now() + std::time::Duration::from_secs(age),
    }
}

#[test]
fn tab_activation_target_history_uses_most_recent_remaining_tab() {
    let documents = [
        activation_doc('a', 0),
        activation_doc('b', 3),
        activation_doc('c', 1),
        activation_doc('d', 2),
    ];

    assert_eq!(
        tab_activation_target_after_close(
            &documents,
            'c',
            Some('c'),
            crate::config::TabActivateOnClose::History,
        ),
        Some('b')
    );
}

#[test]
fn tab_activation_target_neighbour_prefers_right_then_left() {
    let documents = [
        activation_doc('a', 0),
        activation_doc('b', 1),
        activation_doc('c', 2),
        activation_doc('d', 3),
    ];

    assert_eq!(
        tab_activation_target_after_close(
            &documents,
            'b',
            Some('b'),
            crate::config::TabActivateOnClose::Neighbour,
        ),
        Some('c')
    );
    assert_eq!(
        tab_activation_target_after_close(
            &documents,
            'd',
            Some('d'),
            crate::config::TabActivateOnClose::Neighbour,
        ),
        Some('c')
    );
}

#[test]
fn tab_activation_target_left_neighbour_prefers_left_then_right() {
    let documents = [
        activation_doc('a', 0),
        activation_doc('b', 1),
        activation_doc('c', 2),
    ];

    assert_eq!(
        tab_activation_target_after_close(
            &documents,
            'c',
            Some('c'),
            crate::config::TabActivateOnClose::LeftNeighbour,
        ),
        Some('b')
    );
    assert_eq!(
        tab_activation_target_after_close(
            &documents,
            'a',
            Some('a'),
            crate::config::TabActivateOnClose::LeftNeighbour,
        ),
        Some('b')
    );
}

#[test]
fn tab_activation_target_ignores_inactive_or_missing_closes() {
    let documents = [activation_doc('a', 0), activation_doc('b', 1)];

    assert_eq!(
        tab_activation_target_after_close(
            &documents,
            'a',
            Some('b'),
            crate::config::TabActivateOnClose::Neighbour,
        ),
        None
    );
    assert_eq!(
        tab_activation_target_after_close(
            &documents,
            'x',
            Some('x'),
            crate::config::TabActivateOnClose::Neighbour,
        ),
        None
    );
}

fn max_tab_doc(
    id: char,
    age: u64,
    is_modified: bool,
    is_pinned: bool,
    is_protected: bool,
) -> MaxTabsDocument<char> {
    MaxTabsDocument {
        id,
        focused_at: std::time::Instant::now() + std::time::Duration::from_secs(age),
        is_modified,
        is_pinned,
        is_protected,
    }
}

#[test]
fn max_tabs_settings_change_target_allows_active_settings_tab_over_cap() {
    let documents = [
        max_tab_doc('a', 0, false, false, false),
        max_tab_doc('b', 1, false, false, false),
        max_tab_doc('c', 2, false, false, false),
        max_tab_doc('s', 3, false, false, true),
    ];

    assert_eq!(
        max_tabs_close_candidates_to_target(&documents, Some(2)),
        vec!['a', 'b']
    );
    assert_eq!(
        max_tabs_close_candidates_to_target(&documents, Some(3)),
        vec!['a']
    );
}

#[test]
fn max_tabs_close_candidates_preserve_dirty_pinned_and_protected_tabs() {
    let documents = [
        max_tab_doc('a', 0, true, false, false),
        max_tab_doc('b', 1, false, true, false),
        max_tab_doc('c', 2, false, false, true),
        max_tab_doc('d', 3, false, false, false),
    ];

    let close_candidates = max_tabs_close_candidates(&documents, std::num::NonZeroUsize::new(1));

    assert_eq!(close_candidates, vec!['d']);
}

#[test]
fn max_tabs_close_candidates_do_nothing_when_unlimited_or_under_cap() {
    let documents = [
        max_tab_doc('a', 0, false, false, false),
        max_tab_doc('b', 1, false, false, false),
    ];

    assert!(max_tabs_close_candidates(&documents, None).is_empty());
    assert!(max_tabs_close_candidates(&documents, std::num::NonZeroUsize::new(2)).is_empty());
}

fn close_batch_doc(
    id: char,
    path: Option<&'static str>,
    is_active: bool,
) -> BatchCloseDocument<char, &'static str> {
    BatchCloseDocument {
        id,
        is_active,
        path,
    }
}

#[test]
fn batch_close_order_matches_zed_path_sorting() {
    let documents = [
        close_batch_doc('u', None, false),
        close_batch_doc('b', Some("/project/b.rs"), false),
        close_batch_doc('a', Some("/project/a.rs"), false),
        close_batch_doc('m', None, false),
    ];

    assert_eq!(
        batch_close_document_order(&documents),
        vec!['a', 'b', 'u', 'm']
    );
}

#[test]
fn batch_close_order_closes_active_document_last() {
    let documents = [
        close_batch_doc('a', Some("/project/a.rs"), false),
        close_batch_doc('z', Some("/project/z.rs"), true),
        close_batch_doc('b', Some("/project/b.rs"), false),
        close_batch_doc('u', None, false),
    ];

    assert_eq!(
        batch_close_document_order(&documents),
        vec!['a', 'b', 'u', 'z']
    );
}

fn selection_fragments(selection: &Selection, text: &Rope) -> Vec<String> {
    selection
        .fragments(text.slice(..))
        .map(|fragment| fragment.into_owned())
        .collect()
}

#[test]
fn regex_selection_result_selects_matches() {
    let text = Rope::from("one two one");
    let selection = Selection::single(0, text.len_chars());
    let regex = test_regex("one");

    let result = regex_selection_result(
        RegexSelectionAction::Select,
        text.slice(..),
        &selection,
        &regex,
    )
    .unwrap();

    assert_eq!(selection_fragments(&result, &text), vec!["one", "one"]);
}

#[test]
fn regex_selection_result_splits_matches() {
    let text = Rope::from("one,two,three");
    let selection = Selection::single(0, text.len_chars());
    let regex = test_regex(",");

    let result = regex_selection_result(
        RegexSelectionAction::Split,
        text.slice(..),
        &selection,
        &regex,
    )
    .unwrap();

    assert_eq!(
        selection_fragments(&result, &text),
        vec!["one", "two", "three"]
    );
}

#[test]
fn regex_selection_result_keeps_or_removes_matching_selections() {
    let text = Rope::from("one two");
    let selection = Selection::new(
        SmallVec::from_vec(vec![Range::new(0, 3), Range::new(4, 7)]),
        0,
    );
    let regex = test_regex("one");

    let kept = regex_selection_result(
        RegexSelectionAction::Keep,
        text.slice(..),
        &selection,
        &regex,
    )
    .unwrap();
    let removed = regex_selection_result(
        RegexSelectionAction::Remove,
        text.slice(..),
        &selection,
        &regex,
    )
    .unwrap();

    assert_eq!(selection_fragments(&kept, &text), vec!["one"]);
    assert_eq!(selection_fragments(&removed, &text), vec!["two"]);
}

#[test]
fn regex_selection_result_reports_empty_results() {
    let text = Rope::from("one two");
    let selection = Selection::single(0, text.len_chars());
    let regex = test_regex("missing");

    assert_eq!(
        regex_selection_result(
            RegexSelectionAction::Select,
            text.slice(..),
            &selection,
            &regex,
        ),
        Err("nothing selected")
    );
    assert_eq!(
        regex_selection_result(
            RegexSelectionAction::Keep,
            text.slice(..),
            &selection,
            &regex,
        ),
        Err("no selections remaining")
    );
}

#[test]
fn global_search_custom_ignore_filenames_keeps_helix_config_for_local() {
    assert_eq!(
        global_search_custom_ignore_filenames(WorkspaceIdentity::Local),
        vec![helix_loader::config_dir().join("ignore")]
    );
}

#[test]
fn global_search_custom_ignore_filenames_drops_host_config_for_remote() {
    let identity = WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
        kind: nucleotide_workspace::RemoteWorkspaceKind::Wsl,
        name: "Ubuntu".to_string(),
    });

    assert!(global_search_custom_ignore_filenames(identity).is_empty());
}

#[test]
fn global_search_matches_finds_line_matches() {
    let temp_dir = tempfile::tempdir().unwrap();
    let nested_dir = temp_dir.path().join("nested");
    std::fs::create_dir(&nested_dir).unwrap();
    let first = temp_dir.path().join("first.txt");
    let second = nested_dir.join("second.txt");
    std::fs::write(&first, "alpha\nneedle one\nomega\n").unwrap();
    std::fs::write(&second, "needle two\nplain\n").unwrap();

    let matches = global_search_matches(
        temp_dir.path(),
        "needle",
        true,
        &default_file_picker_config(),
        &[],
        10,
    )
    .unwrap();

    assert_eq!(matches.len(), 2);
    assert!(matches.iter().any(|search_match| {
        search_match.path == first
            && search_match.line == 1
            && search_match.line_text == "needle one"
    }));
    assert!(matches.iter().any(|search_match| {
        search_match.path == second
            && search_match.line == 0
            && search_match.line_text == "needle two"
    }));
}

#[test]
fn global_search_matches_excludes_open_documents_from_disk_search() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("buffer.txt");
    std::fs::write(&path, "saved needle\n").unwrap();
    let open_documents = vec![(path.clone(), Rope::from("unsaved needle\n"))];

    let matches = global_search_matches(
        temp_dir.path(),
        "needle",
        true,
        &default_file_picker_config(),
        &open_documents,
        10,
    )
    .unwrap();

    assert_eq!(
        matches,
        vec![GlobalSearchMatch {
            path,
            line: 0,
            line_text: "unsaved needle".to_string(),
        }]
    );
}

#[test]
fn global_search_matches_ignores_open_documents_outside_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_path = outside.path().join("outside.txt");
    std::fs::write(root.path().join("inside.txt"), "plain\n").unwrap();
    std::fs::write(&outside_path, "needle\n").unwrap();
    let open_documents = vec![(outside_path, Rope::from("needle\n"))];

    let matches = global_search_matches(
        root.path(),
        "needle",
        true,
        &default_file_picker_config(),
        &open_documents,
        10,
    )
    .unwrap();

    assert!(matches.is_empty());
}

#[tokio::test]
async fn global_search_disk_matches_with_backend_async_finds_line_matches() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(temp_dir.path().join("file.txt"), "needle\n").unwrap();

    let matches = global_search_disk_matches_with_backend_async(
        local_workspace_backend().as_ref(),
        temp_dir.path().to_path_buf(),
        "needle".to_string(),
        true,
        default_file_picker_config(),
        Vec::new(),
        10,
    )
    .await
    .unwrap();

    assert_eq!(
        matches,
        vec![GlobalSearchMatch {
            path: temp_dir.path().join("file.txt"),
            line: 0,
            line_text: "needle".to_string(),
        }]
    );
}

#[tokio::test]
async fn discover_remote_rust_runnables_uses_workspace_backend_cargo_root() {
    let temp_dir = tempfile::tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir(&src_dir).unwrap();
    std::fs::write(temp_dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    let path = src_dir.join("lib.rs");
    std::fs::write(&path, "#[test]\nfn it_works() {}\n").unwrap();

    let tasks = Workspace::discover_remote_rust_runnables_for_document(
        local_workspace_backend(),
        crate::runnables::RunnableDocument {
            path: path.clone(),
            text: std::fs::read_to_string(&path).unwrap(),
            cursor_line: 1,
            project_root: None,
        },
    )
    .await;

    assert!(tasks.iter().any(|task| task.label() == "Run Test it_works"));
}

#[test]
fn file_picker_items_from_backend_uses_workspace_file_search() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp_dir.path().join("src")).unwrap();
    std::fs::create_dir(temp_dir.path().join("zed-source")).unwrap();
    std::fs::write(temp_dir.path().join("src").join("main.rs"), "").unwrap();
    std::fs::write(temp_dir.path().join(".hidden"), "").unwrap();
    std::fs::write(temp_dir.path().join("zed-source").join("skip.rs"), "").unwrap();

    let backend = local_workspace_backend();
    let items = file_picker_items_from_backend(
        backend.as_ref(),
        temp_dir.path(),
        &default_file_picker_config(),
    )
    .unwrap();
    let labels = items
        .iter()
        .map(|item| item.label.to_string())
        .collect::<Vec<_>>();

    assert_eq!(labels, vec!["src/main.rs"]);
    assert_eq!(
        items[0].file_path.as_ref(),
        Some(&temp_dir.path().join("src").join("main.rs"))
    );
}

#[test]
fn file_picker_items_from_backend_respects_hidden_config() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(temp_dir.path().join(".hidden"), "").unwrap();

    let backend = local_workspace_backend();
    let mut config = default_file_picker_config();
    config.hidden = false;
    let items = file_picker_items_from_backend(backend.as_ref(), temp_dir.path(), &config).unwrap();
    let labels = items
        .iter()
        .map(|item| item.label.to_string())
        .collect::<Vec<_>>();

    assert_eq!(labels, vec![".hidden"]);
}

#[tokio::test]
async fn file_picker_search_keeps_candidates_beyond_one_thousand() {
    let temp_dir = tempfile::tempdir().unwrap();
    let expected = (0..1_105)
        .map(|index| PathBuf::from(format!("file_{index:04}.rs")))
        .collect::<Vec<_>>();
    for path in expected.iter().rev() {
        std::fs::write(temp_dir.path().join(path), "").unwrap();
    }
    let result = file_picker_search_result_from_backend(
        local_workspace_backend(),
        temp_dir.path().to_path_buf(),
        default_file_picker_config(),
    )
    .await
    .unwrap();
    assert_eq!(result.files, expected);
    assert!(!result.truncated);
}

#[tokio::test]
async fn file_picker_preview_from_backend_async_reads_bounded_text() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("large.txt");
    std::fs::write(
        &path,
        "a".repeat(FILE_PICKER_PREVIEW_MAX_BYTES as usize + 128),
    )
    .unwrap();

    let backend = local_workspace_backend();
    let preview = file_picker_preview_text_from_backend_async(backend, path).await;

    assert!(preview.starts_with(&"a".repeat(FILE_PICKER_PREVIEW_MAX_BYTES as usize)));
    assert!(preview.contains("[File truncated - showing first 10KB"));
}

#[test]
fn file_picker_preview_decode_keeps_text_truncated_mid_codepoint() {
    let bytes = "hello ".as_bytes().iter().copied().chain([0xE2, 0x82]);
    let bytes = bytes.collect::<Vec<_>>();

    let preview = file_picker_decode_preview_text(&bytes, true);

    assert_eq!(preview.as_deref(), Some("hello "));
}

#[test]
fn file_picker_preview_from_backend_formats_directory_listing() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp_dir.path().join("src")).unwrap();
    std::fs::write(temp_dir.path().join("README.md"), "").unwrap();

    let backend = local_workspace_backend();
    let preview = file_picker_preview_text_from_backend(backend.as_ref(), temp_dir.path());

    assert!(preview.contains("Directory:"));
    assert!(preview.contains("README.md"));
    assert!(preview.contains("src/"));
}
