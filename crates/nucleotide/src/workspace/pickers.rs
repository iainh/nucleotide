//! File, directory, buffer, and code-action picker construction and previews.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, Entity};
use helix_core::syntax::config::LanguageServerFeature;
use helix_lsp::lsp;
use nucleotide_logging::{debug, info, warn};
#[cfg(test)]
use nucleotide_workspace::WorkspaceBackend;
use nucleotide_workspace::{
    FileKind, FileSearchQuery, FileSearchResult, FileStat, ReadOptions, WorkspaceBackendHandle,
    WorkspaceIdentity, absolutize_workspace_path, posix_path_string,
};

use super::{Workspace, document_lsp_identifier};
use crate::Core;
use crate::overlay::OverlayView;

pub(super) fn open(
    core: Entity<Core>,
    handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    cx: &mut Context<Workspace>,
) {
    let (base_dir, workspace_backend, file_picker_config) = core.update(cx, |core, _| {
        (
            core.project_directory
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            core.workspace_backend.clone(),
            core.editor.config().file_picker.clone(),
        )
    });

    open_at(
        core,
        handle,
        overlay,
        base_dir,
        workspace_backend,
        file_picker_config,
        cx,
    );
}

pub(super) const FILE_PICKER_PREVIEW_MAX_BYTES: u64 = 10 * 1024;
const FILE_PICKER_PREVIEW_MAX_DIR_ENTRIES: usize = 100;

pub(super) fn open_at(
    _core: Entity<Core>,
    handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    base_dir: std::path::PathBuf,
    workspace_backend: WorkspaceBackendHandle,
    file_picker_config: helix_view::editor::FilePickerConfig,
    cx: &mut Context<Workspace>,
) {
    debug!("Opening file picker");

    debug!("Base directory for file picker: {:?}", base_dir);

    let workspace_identity = workspace_backend.identity();
    info!(
        workspace_root = %base_dir.display(),
        backend = ?workspace_identity,
        "Loading file picker items through workspace backend"
    );

    let overlay = overlay.clone();
    let search_base_dir = base_dir.clone();
    let item_base_dir = base_dir.clone();
    let picker_backend = workspace_backend.clone();
    let picker_handle = handle.clone();
    cx.spawn(async move |this, cx| {
        let search_result = match handle
            .spawn(file_picker_search_result_from_backend(
                workspace_backend,
                search_base_dir,
                file_picker_config,
            ))
            .await
        {
            Ok(result) => result,
            Err(err) => Err(err.to_string()),
        };

        if let Some(this) = this.upgrade() {
            this.update(cx, |_workspace, cx| {
                let items = match search_result {
                    Ok(result) => file_picker_items_from_search_result(&item_base_dir, result),
                    Err(err) => {
                        warn!(
                            workspace_root = %item_base_dir.display(),
                            error = %err,
                            "Failed to build file picker items"
                        );
                        Vec::new()
                    }
                };
                emit_file_picker_from_items(items, picker_backend, picker_handle, overlay, cx);
            });
        }
    })
    .detach();
}

fn emit_file_picker_from_items(
    mut items: Vec<crate::picker_view::PickerItem>,
    workspace_backend: WorkspaceBackendHandle,
    handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    cx: &mut Context<Workspace>,
) {
    // Sort items by label (path) for consistent ordering
    items.sort_by_key(|item| item.label.clone());

    debug!("File picker has {} items", items.len());

    // Populate VCS status for all file items using the global VCS service
    if let Some(vcs_service) = cx.try_global::<nucleotide_vcs::VcsServiceHandle>() {
        debug!("VCS service available, populating file picker VCS status");

        // Apply VCS status to items using cached status
        let mut vcs_status_count = 0;
        for item in &mut items {
            if let Some(ref file_path) = item.file_path {
                item.vcs_status = vcs_service.get_status_cached(file_path, cx);
                if item.vcs_status.is_some() {
                    vcs_status_count += 1;
                }
            }
        }

        debug!(
            file_count = items.len(),
            vcs_status_count = vcs_status_count,
            "Populated file picker VCS status"
        );
    } else {
        debug!("VCS service not available");
    }

    let workspace_identity = workspace_backend.identity();

    // Create a simple native picker without callback - the overlay will handle file opening via events
    let mut file_picker = crate::picker::Picker::native("Open File", items, |_index| {
        // This callback is no longer used - file opening is handled via OpenFile events
        // The overlay will emit OpenFile events when files are selected
    })
    .with_preview(true);

    if !matches!(workspace_identity, WorkspaceIdentity::Local) {
        let preview_backend = workspace_backend.clone();
        let preview_handle = handle.clone();
        file_picker = file_picker.with_preview_text_task_provider_fn(move |item, cx| {
            let path = item.file_path.clone()?;
            let backend = preview_backend.clone();
            let handle = preview_handle.clone();
            Some(cx.spawn(async move |_view, _cx| {
                let preview_path = path.clone();
                match handle
                    .spawn(file_picker_preview_text_from_backend_async(
                        backend,
                        preview_path.clone(),
                    ))
                    .await
                {
                    Ok(text) => Some((text, Some(preview_path))),
                    Err(err) => Some((format!("Error reading file: {err}"), Some(preview_path))),
                }
            }))
        });
    }

    debug!("Emitting file picker to overlay");

    emit_picker_update(file_picker, &overlay, cx);
}

#[cfg(test)]
pub(super) fn file_picker_preview_text_from_backend(
    backend: &(impl WorkspaceBackend + ?Sized),
    path: &Path,
) -> String {
    match futures_executor::block_on(backend.stat(path)) {
        Ok(stat) => match stat.kind {
            FileKind::Directory => file_picker_directory_preview_from_backend(backend, path),
            FileKind::File | FileKind::Symlink => {
                file_picker_file_preview_from_backend(backend, path, &stat)
            }
            FileKind::Other => file_picker_binary_preview(path, &stat),
        },
        Err(err) => format!("Error reading file: {err}"),
    }
}

pub(super) async fn file_picker_preview_text_from_backend_async(
    backend: WorkspaceBackendHandle,
    path: PathBuf,
) -> String {
    match backend.stat(&path).await {
        Ok(stat) => match stat.kind {
            FileKind::Directory => {
                file_picker_directory_preview_from_backend_async(backend, path).await
            }
            FileKind::File | FileKind::Symlink => {
                file_picker_file_preview_from_backend_async(backend, path, stat).await
            }
            FileKind::Other => file_picker_binary_preview(&path, &stat),
        },
        Err(err) => format!("Error reading file: {err}"),
    }
}

#[cfg(test)]
fn file_picker_directory_preview_from_backend(
    backend: &(impl WorkspaceBackend + ?Sized),
    path: &Path,
) -> String {
    match futures_executor::block_on(backend.list_dir(path)) {
        Ok(listing) => file_picker_directory_preview_from_listing(path, &listing.entries),
        Err(err) => format!("Error reading directory: {err}"),
    }
}

async fn file_picker_directory_preview_from_backend_async(
    backend: WorkspaceBackendHandle,
    path: PathBuf,
) -> String {
    match backend.list_dir(&path).await {
        Ok(listing) => file_picker_directory_preview_from_listing(&path, &listing.entries),
        Err(err) => format!("Error reading directory: {err}"),
    }
}

fn file_picker_directory_preview_from_listing(
    path: &Path,
    entries: &[nucleotide_workspace::DirectoryEntry],
) -> String {
    let mut content = format!("Directory: {}\n\n", path.display());
    for entry in entries.iter().take(FILE_PICKER_PREVIEW_MAX_DIR_ENTRIES) {
        let suffix = if matches!(entry.stat.kind, FileKind::Directory) {
            "/"
        } else {
            ""
        };
        content.push_str(&format!("{}{}\n", entry.name, suffix));
    }
    if entries.len() > FILE_PICKER_PREVIEW_MAX_DIR_ENTRIES {
        content.push_str(&format!(
            "\n... and {} more items\n",
            entries.len() - FILE_PICKER_PREVIEW_MAX_DIR_ENTRIES
        ));
    }
    content
}

#[cfg(test)]
fn file_picker_file_preview_from_backend(
    backend: &(impl WorkspaceBackend + ?Sized),
    path: &Path,
    stat: &FileStat,
) -> String {
    match futures_executor::block_on(backend.read_file(
        path,
        ReadOptions {
            max_bytes: Some(FILE_PICKER_PREVIEW_MAX_BYTES),
        },
    )) {
        Ok(read) => {
            file_picker_file_preview_from_read(path, stat, &read.bytes, read.truncated, read.size)
        }
        Err(err) => format!("Error reading file: {err}"),
    }
}

async fn file_picker_file_preview_from_backend_async(
    backend: WorkspaceBackendHandle,
    path: PathBuf,
    stat: FileStat,
) -> String {
    match backend
        .read_file(
            &path,
            ReadOptions {
                max_bytes: Some(FILE_PICKER_PREVIEW_MAX_BYTES),
            },
        )
        .await
    {
        Ok(read) => {
            file_picker_file_preview_from_read(&path, &stat, &read.bytes, read.truncated, read.size)
        }
        Err(err) => format!("Error reading file: {err}"),
    }
}

fn file_picker_file_preview_from_read(
    path: &Path,
    stat: &FileStat,
    bytes: &[u8],
    truncated: bool,
    size: u64,
) -> String {
    match file_picker_decode_preview_text(bytes, truncated) {
        Some(mut content) => {
            if truncated {
                content.push_str(&format!(
                    "\n\n[File truncated - showing first {}KB of {}KB total]",
                    FILE_PICKER_PREVIEW_MAX_BYTES / 1024,
                    size / 1024
                ));
            }
            content
        }
        None => file_picker_binary_preview(path, stat),
    }
}

pub(super) fn file_picker_decode_preview_text(bytes: &[u8], truncated: bool) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_string()),
        Err(err) if truncated && err.error_len().is_none() => {
            Some(String::from_utf8_lossy(&bytes[..err.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

fn file_picker_binary_preview(path: &Path, stat: &FileStat) -> String {
    format!(
        "Binary file: {}\nSize: {} bytes\nModified: {:?}",
        path.display(),
        stat.size,
        stat.modified.unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    )
}

#[cfg(test)]
pub(super) fn file_picker_items_from_backend(
    backend: &(impl WorkspaceBackend + ?Sized),
    base_dir: &Path,
    file_picker_config: &helix_view::editor::FilePickerConfig,
) -> Result<Vec<crate::picker_view::PickerItem>, String> {
    let result = futures_executor::block_on(
        backend.file_search(file_picker_search_query(base_dir, file_picker_config)),
    )
    .map_err(|err| err.to_string())?;

    Ok(file_picker_items_from_search_result(base_dir, result))
}

pub(super) async fn file_picker_search_result_from_backend(
    backend: WorkspaceBackendHandle,
    base_dir: PathBuf,
    file_picker_config: helix_view::editor::FilePickerConfig,
) -> Result<FileSearchResult, String> {
    let stream = backend
        .file_search_stream(file_picker_search_query(&base_dir, &file_picker_config))
        .await
        .map_err(|err| err.to_string())?;
    stream
        .collect_search(&base_dir)
        .await
        .map_err(|err| err.to_string())
}

fn file_picker_search_query(
    base_dir: &Path,
    file_picker_config: &helix_view::editor::FilePickerConfig,
) -> FileSearchQuery {
    FileSearchQuery {
        root: base_dir.to_path_buf(),
        pattern: None,
        // Filtering happens in PickerView; every candidate must be searchable.
        limit: usize::MAX,
        hidden: !file_picker_config.hidden,
        parents: file_picker_config.parents,
        ignore: file_picker_config.ignore,
        git_ignore: file_picker_config.git_ignore,
        git_global: file_picker_config.git_global,
        git_exclude: file_picker_config.git_exclude,
        follow_links: file_picker_config.follow_symlinks,
        max_depth: file_picker_config.max_depth,
        excluded_relative_prefixes: vec![PathBuf::from("zed-source")],
    }
}

fn file_picker_items_from_search_result(
    base_dir: &Path,
    result: FileSearchResult,
) -> Vec<crate::picker_view::PickerItem> {
    use crate::picker_view::PickerItem;
    use std::sync::Arc;

    result
        .files
        .into_iter()
        .map(|relative_path| {
            let path = absolutize_workspace_path(base_dir, &relative_path);
            let label = posix_path_string(&relative_path);
            PickerItem {
                label: label.into(),
                sublabel: None,
                data: Arc::new(path.clone()) as Arc<dyn std::any::Any + Send + Sync>,
                file_path: Some(path),
                vcs_status: None,
                columns: None,
            }
        })
        .collect()
}

pub(super) fn open_directory(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    debug!("Opening directory picker");

    // Create a native directory picker
    let directory_picker =
        crate::picker::Picker::native_directory("Select Project Directory", |path| {
            info!("Directory selected: {:?}", path);
            // The callback will be handled through events
        });

    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::DirectoryPicker(directory_picker));
    });
}

pub(super) fn show_buffer_picker(
    core: Entity<Core>,
    _handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    cx: &mut Context<Workspace>,
) {
    use crate::picker_view::PickerItem;
    use helix_view::DocumentId;
    use std::sync::Arc;

    debug!("Opening buffer picker");

    // Structure to hold buffer metadata for sorting
    #[derive(Clone)]
    struct BufferMeta {
        doc_id: DocumentId,
        path: Option<std::path::PathBuf>,
        is_modified: bool,
        is_current: bool,
        focused_at: std::time::Instant,
    }

    let (project_directory, mut buffer_metas) = {
        let core = core.read(cx);
        let editor = &core.editor;
        let current_doc_id = editor
            .tree
            .try_get(editor.tree.focus)
            .map(|view| view.doc)
            .unwrap_or_else(|| editor.documents.keys().next().copied().unwrap_or_default());

        let buffer_metas: Vec<BufferMeta> = editor
            .documents
            .iter()
            .map(|(doc_id, doc)| BufferMeta {
                doc_id: *doc_id,
                path: doc.path().map(|p| p.to_path_buf()),
                is_modified: doc.is_modified(),
                is_current: *doc_id == current_doc_id,
                focused_at: doc.focused_at,
            })
            .collect();

        (core.project_directory.clone(), buffer_metas)
    };

    // Sort by MRU (Most Recently Used) - most recent first
    buffer_metas.sort_by_key(|meta| std::cmp::Reverse(meta.focused_at));

    // Create picker items with terminal-like formatting
    let mut items = Vec::new();

    for meta in buffer_metas {
        // Format like terminal: "ID  FLAGS  PATH"
        // DocumentId likely has Display impl that shows "DocumentId(N)"
        let display_str = format!("{}", meta.doc_id);

        // Extract number from "DocumentId(N)" format
        let id_str = if display_str.starts_with("DocumentId(") && display_str.ends_with(")") {
            // Extract the number between parentheses
            display_str[11..display_str.len() - 1].to_string()
        } else if let Some(start) = display_str.find('(') {
            // More flexible parsing for variations
            if let Some(end) = display_str.rfind(')') {
                display_str[start + 1..end].trim().to_string()
            } else {
                display_str[start + 1..].trim().to_string()
            }
        } else if display_str.chars().all(char::is_numeric) {
            // If it's already just a number, use it
            display_str
        } else {
            // Fallback - try to find any number in the string
            display_str
                .chars()
                .skip_while(|c| !c.is_numeric())
                .take_while(|c| c.is_numeric())
                .collect::<String>()
        };

        // Build flags column - ensure consistent 2-character width
        let mut flags = String::new();
        if meta.is_modified {
            flags.push('+');
        }
        if meta.is_current {
            flags.push('*');
        }

        // Ensure flags are always exactly 2 characters for consistent column alignment
        let flags_str = format!("{flags:2}");

        // Get path or [scratch] label
        let path_str = if let Some(path) = &meta.path {
            // Show relative path if possible
            if let Some(project_dir) = &project_directory {
                path.strip_prefix(project_dir)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            } else {
                path.display().to_string()
            }
        } else {
            "[scratch]".to_string()
        };

        // Create data that includes both doc_id and path for preview functionality
        // We'll store a tuple of (DocumentId, Option<PathBuf>) for all items
        let picker_data =
            Arc::new((meta.doc_id, meta.path.clone())) as Arc<dyn std::any::Any + Send + Sync>;

        // Use structured columns instead of text formatting
        items.push(PickerItem::with_buffer_columns(
            id_str,
            flags_str,
            path_str,
            picker_data,
        ));
    }

    debug!("Buffer picker has {} items", items.len());

    // Create the picker with buffer items
    let buffer_picker = crate::picker::Picker::native("Switch Buffer", items, move |index| {
        debug!("Buffer selected at index: {}", index);
        // The overlay will handle buffer switching via the stored document ID
    })
    .with_preview(true);

    emit_picker_update(buffer_picker, &overlay, cx);
}

pub(super) fn emit_picker_update(
    picker: crate::picker::Picker,
    overlay: &Entity<OverlayView>,
    cx: &mut Context<Workspace>,
) {
    let update = crate::Update::Picker(picker);
    overlay.update(cx, |overlay, cx| overlay.handle_event(&update, cx));
    cx.emit(update);
}

fn code_action_category(action: &lsp::CodeActionOrCommand) -> u32 {
    if let lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
        kind: Some(kind), ..
    }) = action
    {
        let mut components = kind.as_str().split('.');
        match components.next() {
            Some("quickfix") => 0,
            Some("refactor") => match components.next() {
                Some("extract") => 1,
                Some("inline") => 2,
                Some("rewrite") => 3,
                Some("move") => 4,
                Some("surround") => 5,
                _ => 7,
            },
            Some("source") => 6,
            _ => 7,
        }
    } else {
        7
    }
}

fn code_action_preferred(action: &lsp::CodeActionOrCommand) -> bool {
    matches!(
        action,
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            is_preferred: Some(true),
            ..
        })
    )
}

fn code_action_fixes_diagnostics(action: &lsp::CodeActionOrCommand) -> bool {
    matches!(
        action,
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction { diagnostics: Some(diags), .. }) if !diags.is_empty()
    )
}

pub(super) fn code_action_enabled(action: &lsp::CodeActionOrCommand) -> bool {
    matches!(
        action,
        lsp::CodeActionOrCommand::Command(_)
            | lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction { disabled: None, .. })
    )
}

fn code_action_label(action: &lsp::CodeActionOrCommand) -> &str {
    match action {
        lsp::CodeActionOrCommand::Command(command) => command.title.as_str(),
        lsp::CodeActionOrCommand::CodeAction(code_action) => code_action.title.as_str(),
    }
}

pub(super) fn code_action_metadata_label(
    action: &lsp::CodeActionOrCommand,
    server_name: &str,
) -> String {
    let mut parts = Vec::new();

    match action {
        lsp::CodeActionOrCommand::Command(command) => {
            parts.push(format!("command: {}", command.command));
        }
        lsp::CodeActionOrCommand::CodeAction(code_action) => {
            parts.push(
                code_action
                    .kind
                    .as_ref()
                    .map(|kind| kind.as_str().to_string())
                    .unwrap_or_else(|| "code action".to_string()),
            );

            if code_action.is_preferred == Some(true) {
                parts.push("preferred".to_string());
            }
            if code_action_fixes_diagnostics(action) {
                parts.push("fixes diagnostics".to_string());
            }
            if code_action.data.is_some()
                && (code_action.edit.is_none() || code_action.command.is_none())
            {
                parts.push("resolves on apply".to_string());
            }
        }
    }

    parts.push(server_name.to_string());
    parts.join(" · ")
}

pub(super) fn sort_code_actions_like_helix(actions: &mut [lsp::CodeActionOrCommand]) {
    actions.sort_by(|a, b| {
        let category = code_action_category(a).cmp(&code_action_category(b));
        if category != std::cmp::Ordering::Equal {
            return category;
        }

        let fixes_diagnostic = code_action_fixes_diagnostics(a)
            .cmp(&code_action_fixes_diagnostics(b))
            .reverse();
        if fixes_diagnostic != std::cmp::Ordering::Equal {
            return fixes_diagnostic;
        }

        code_action_preferred(a)
            .cmp(&code_action_preferred(b))
            .reverse()
    });
}

pub(super) fn show_code_actions(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    use futures_util::stream::{FuturesOrdered, StreamExt};
    use helix_lsp::lsp;
    use helix_lsp::util::{diagnostic_to_lsp_diagnostic, range_to_lsp_range};

    debug!("Opening code actions dropdown");

    // Snapshot needed editor state under read lock
    let Some((identifier, selection_range, doc_text, diags, servers)) = (|| {
        let core_r = core.read(cx);
        let editor = &core_r.editor;
        let view = editor.tree.try_get(editor.tree.focus)?;
        let doc = editor.documents.get(&view.doc)?;

        let selection_range = doc.selection(view.id).primary();
        let doc_text = doc.text().clone();
        let diags = doc
            .diagnostics()
            .iter()
            .filter(|d| {
                selection_range.overlaps(&helix_core::Range::new(d.range.start, d.range.end))
            })
            .cloned()
            .collect::<Vec<_>>();

        // Collect unique servers supporting CodeAction
        let mut seen = std::collections::HashSet::new();
        let servers: Vec<_> = doc
            .language_servers_with_feature(LanguageServerFeature::CodeAction)
            .filter(|ls| seen.insert(ls.id()))
            .collect();

        let identifier = document_lsp_identifier(doc)?;

        Some((identifier, selection_range, doc_text, diags, servers))
    })() else {
        core.update(cx, |core, cx| {
            core.editor
                .set_error("Code actions require a file-backed document");
            cx.notify();
        });
        return;
    };

    if servers.is_empty() {
        debug!("No language servers with CodeAction support");
        core.update(cx, |core, cx| {
            core.editor
                .set_error("No configured language server supports code actions");
            cx.notify();
        });
        return;
    }

    let mut futures: FuturesOrdered<_> = servers
        .into_iter()
        .filter_map(|ls| {
            let offset = ls.offset_encoding();
            let ls_id = ls.id();
            let server_name = ls.name().to_string();
            let range = range_to_lsp_range(&doc_text, selection_range, offset);
            let ctx = lsp::CodeActionContext {
                diagnostics: diags
                    .iter()
                    .map(|d| diagnostic_to_lsp_diagnostic(&doc_text, d, offset))
                    .collect(),
                only: None,
                trigger_kind: Some(lsp::CodeActionTriggerKind::INVOKED),
            };
            let req = ls.code_actions(identifier.clone(), range, ctx)?;
            Some(async move {
                req.await
                    .map(|opt| (opt.unwrap_or_default(), ls_id, offset, server_name))
            })
        })
        .collect();

    if futures.is_empty() {
        core.update(cx, |core, cx| {
            core.editor
                .set_error("No configured language server supports code actions");
            cx.notify();
        });
        return;
    }

    // Spawn async collection job
    let core_weak = core.downgrade();
    cx.spawn(async move |cx| {
        let mut items = Vec::new();

        while let Some(result) = futures.next().await {
            match result {
                Ok((mut actions, ls_id, offset, server_name)) => {
                    // Drop disabled actions
                    actions.retain(code_action_enabled);

                    // Sort as in Helix: category, then fixes diagnostics, then preferred
                    sort_code_actions_like_helix(&mut actions);

                    for action in actions.into_iter() {
                        let label = code_action_label(&action).to_string();
                        let sublabel = code_action_metadata_label(&action, &server_name);

                        items.push(crate::picker_view::PickerItem {
                            label: label.into(),
                            sublabel: Some(sublabel.into()),
                            data: Arc::new((action, ls_id, offset)),
                            file_path: None,
                            vcs_status: None,
                            columns: None,
                        });
                    }
                }
                Err(err) => {
                    warn!(error = %err, "Error collecting code actions from server");
                }
            }
        }

        // If none, exit with a notification
        if items.is_empty() {
            if let Some(core) = core_weak.upgrade() {
                core.update(cx, |core, cx| {
                    core.editor.set_error("No code actions available");
                    cx.emit(crate::Update::EditorStatus(
                        nucleotide_types::EditorStatus {
                            status: "No code actions available".to_string(),
                            severity: nucleotide_types::Severity::Error,
                        },
                    ));
                });
            }
            return;
        }

        if let Some(core) = core_weak.upgrade() {
            let picker = crate::picker::Picker::native("Code Actions", items, |_index| {
                // Selection is handled by OverlayView via typed code-action payloads.
            });
            core.update(cx, |_core, cx| {
                cx.emit(crate::Update::Picker(picker));
            });
        }
    })
    .detach();
}

impl Workspace {
    /// Open file picker
    pub fn open_file_picker(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Opening file picker");

        let handle = self.handle.clone();
        let core = self.core.clone();
        let overlay = self.overlay.clone();
        open(core, handle, overlay, cx);
    }
}
