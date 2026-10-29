//! Local and remote LSP workspace edits and their asynchronous operation queue.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::task::{Context as TaskContext, Poll};

use futures_util::future::{BoxFuture, FutureExt};
use helix_core::Uri;
use helix_lsp::{LanguageServerId, OffsetEncoding, lsp};
use helix_view::{Document, DocumentId, Editor};
use nucleotide_logging::error;
use nucleotide_workspace::{
    FileStat, WorkspaceBackendHandle, WorkspaceError, WorkspaceIdentity, WriteOptions,
};

use super::{
    Application, MAINTENANCE_LSP_COMMAND_BATCH, WorkspaceDocumentRead, navigation_display_path,
    open_workspace_document, open_workspace_document_from_read, read_workspace_document,
    remote_lsp_file_url_path, set_remote_document_lsp_url,
};

type ApplyEditError = helix_view::handlers::lsp::ApplyEditError;

type ApplyEditErrorKind = helix_view::handlers::lsp::ApplyEditErrorKind;

fn apply_edit_io_error(message: impl Into<String>) -> ApplyEditErrorKind {
    ApplyEditErrorKind::IoError(std::io::Error::other(message.into()))
}

fn workspace_error_to_apply_edit(error: WorkspaceError) -> ApplyEditErrorKind {
    apply_edit_io_error(error.to_string())
}

fn workspace_error_has_io_kind(error: &WorkspaceError, kind: std::io::ErrorKind) -> bool {
    match error {
        WorkspaceError::Io { source, .. } => source.kind() == kind,
        WorkspaceError::Remote { message, .. } => match kind {
            std::io::ErrorKind::NotFound => {
                let message = message.to_ascii_lowercase();
                message.contains("not found") || message.contains("no such file")
            }
            std::io::ErrorKind::AlreadyExists => {
                let message = message.to_ascii_lowercase();
                message.contains("already exists") || message.contains("file exists")
            }
            _ => false,
        },
        _ => false,
    }
}

fn workspace_edit_path_from_url(
    uri: &lsp::Url,
    project_directory: Option<&Path>,
    workspace_identity: &WorkspaceIdentity,
) -> Result<PathBuf, ApplyEditErrorKind> {
    if matches!(workspace_identity, WorkspaceIdentity::Remote(_))
        && let Some(native_path) = remote_lsp_file_url_path(uri)
    {
        return Ok(navigation_display_path(
            &native_path,
            project_directory,
            workspace_identity,
        ));
    }

    let uri = Uri::try_from(uri.clone())?;
    let path = uri
        .as_path()
        .map(Path::to_path_buf)
        .ok_or_else(|| apply_edit_io_error(format!("URI does not contain a file path: {uri}")))?;

    Ok(navigation_display_path(
        &path,
        project_directory,
        workspace_identity,
    ))
}

fn apply_workspace_text_edits_with_backend(
    editor: &mut Editor,
    workspace_backend: &WorkspaceBackendHandle,
    project_directory: Option<&Path>,
    uri: &lsp::Url,
    version: Option<i32>,
    text_edits: Vec<lsp::TextEdit>,
    offset_encoding: OffsetEncoding,
) -> Result<(), ApplyEditErrorKind> {
    let workspace_identity = workspace_backend.identity();
    let path = workspace_edit_path_from_url(uri, project_directory, &workspace_identity)?;
    let action = if editor.tree.views().count() == 0 {
        helix_view::editor::Action::VerticalSplit
    } else {
        helix_view::editor::Action::Load
    };
    let doc_id = match open_workspace_document(editor, workspace_backend, &path, action) {
        Ok(doc_id) => doc_id,
        Err(err) => {
            let error = format!("failed to open document: {}: {err}", path.display());
            error!("{error}");
            editor.set_error(error);
            return Err(ApplyEditErrorKind::FileNotFound);
        }
    };

    if let Some(version) = version {
        let doc = editor
            .document(doc_id)
            .ok_or_else(|| apply_edit_io_error(format!("document {doc_id:?} is not open")))?;
        if version != doc.version() {
            let error = format!("outdated workspace edit for {path:?}");
            error!("{error}, expected {} but got {version}", doc.version());
            editor.set_error(error);
            return Err(ApplyEditErrorKind::DocumentChanged);
        }
    }

    let view_id = editor.get_synced_view_id(doc_id);
    let tree = &mut editor.tree;
    let documents = &mut editor.documents;
    let view = tree.get_mut(view_id);
    let doc = documents
        .get_mut(&doc_id)
        .ok_or_else(|| apply_edit_io_error(format!("document {doc_id:?} is not open")))?;
    let transaction =
        helix_lsp::util::generate_transaction_from_edits(doc.text(), text_edits, offset_encoding);
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    Ok(())
}

fn apply_workspace_text_edits_to_open_document(
    editor: &mut Editor,
    doc_id: DocumentId,
    version: Option<i32>,
    text_edits: Vec<lsp::TextEdit>,
    offset_encoding: OffsetEncoding,
) -> Result<(), ApplyEditErrorKind> {
    if let Some(version) = version {
        let doc = editor
            .document(doc_id)
            .ok_or_else(|| apply_edit_io_error(format!("document {doc_id:?} is not open")))?;
        if version != doc.version() {
            return Err(ApplyEditErrorKind::DocumentChanged);
        }
    }

    let view_id = editor.get_synced_view_id(doc_id);
    let tree = &mut editor.tree;
    let documents = &mut editor.documents;
    let view = tree.get_mut(view_id);
    let doc = documents
        .get_mut(&doc_id)
        .ok_or_else(|| apply_edit_io_error(format!("document {doc_id:?} is not open")))?;
    let transaction =
        helix_lsp::util::generate_transaction_from_edits(doc.text(), text_edits, offset_encoding);
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    Ok(())
}

fn lsp_workspace_edit_operations(
    workspace_edit: lsp::WorkspaceEdit,
) -> VecDeque<LspWorkspaceEditOperation> {
    let mut operations = VecDeque::new();
    if let Some(document_changes) = workspace_edit.document_changes {
        match document_changes {
            lsp::DocumentChanges::Edits(document_edits) => {
                for document_edit in document_edits {
                    operations.push_back(LspWorkspaceEditOperation::Text {
                        uri: document_edit.text_document.uri,
                        version: document_edit.text_document.version,
                        edits: document_edit
                            .edits
                            .into_iter()
                            .map(|edit| match edit {
                                lsp::OneOf::Left(text_edit) => text_edit,
                                lsp::OneOf::Right(annotated) => annotated.text_edit,
                            })
                            .collect(),
                    });
                }
            }
            lsp::DocumentChanges::Operations(document_operations) => {
                for operation in document_operations {
                    match operation {
                        lsp::DocumentChangeOperation::Op(resource_op) => {
                            operations.push_back(LspWorkspaceEditOperation::Resource(resource_op))
                        }
                        lsp::DocumentChangeOperation::Edit(document_edit) => {
                            operations.push_back(LspWorkspaceEditOperation::Text {
                                uri: document_edit.text_document.uri,
                                version: document_edit.text_document.version,
                                edits: document_edit
                                    .edits
                                    .into_iter()
                                    .map(|edit| match edit {
                                        lsp::OneOf::Left(text_edit) => text_edit,
                                        lsp::OneOf::Right(annotated) => annotated.text_edit,
                                    })
                                    .collect(),
                            });
                        }
                    }
                }
            }
        }
    } else if let Some(changes) = workspace_edit.changes {
        for (uri, edits) in changes {
            operations.push_back(LspWorkspaceEditOperation::Text {
                uri,
                version: None,
                edits,
            });
        }
    }
    operations
}

fn update_open_document_after_workspace_rename(
    editor: &mut Editor,
    old_path: &Path,
    new_stat: &FileStat,
) {
    let Some(doc_id) = editor.document_by_path(old_path).map(|doc| doc.id()) else {
        return;
    };
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    doc.set_path_with_metadata(
        Some(&new_stat.path),
        Some(new_stat.readonly),
        new_stat.modified,
    );
    doc.set_file_version(
        new_stat
            .version
            .as_ref()
            .map(|version| version.as_bytes().to_vec()),
    );
    set_remote_document_lsp_url(doc, &new_stat.path);
}

fn create_workspace_edit_file(
    workspace_backend: &WorkspaceBackendHandle,
    path: &Path,
    options: Option<&lsp::CreateFileOptions>,
) -> Result<(), ApplyEditErrorKind> {
    let overwrite = options
        .and_then(|options| options.overwrite)
        .unwrap_or(false);
    let ignore_if_exists = options
        .and_then(|options| options.ignore_if_exists)
        .unwrap_or(false);

    if overwrite {
        futures_executor::block_on(workspace_backend.write_file(
            path,
            &[],
            WriteOptions {
                create_parent_dirs: true,
                expected_modified: None,
                expected_version: None,
            },
        ))
        .map_err(workspace_error_to_apply_edit)?;
        return Ok(());
    }

    match futures_executor::block_on(workspace_backend.create_file(path)) {
        Ok(_) => Ok(()),
        Err(error)
            if ignore_if_exists
                && workspace_error_has_io_kind(&error, std::io::ErrorKind::AlreadyExists) =>
        {
            Ok(())
        }
        Err(error) => Err(workspace_error_to_apply_edit(error)),
    }
}

fn delete_workspace_edit_path(
    workspace_backend: &WorkspaceBackendHandle,
    path: &Path,
    options: Option<&lsp::DeleteFileOptions>,
) -> Result<(), ApplyEditErrorKind> {
    let ignore_if_not_exists = options
        .and_then(|options| options.ignore_if_not_exists)
        .unwrap_or(false);

    match futures_executor::block_on(workspace_backend.delete_path(path)) {
        Ok(_) => Ok(()),
        Err(error)
            if ignore_if_not_exists
                && workspace_error_has_io_kind(&error, std::io::ErrorKind::NotFound) =>
        {
            Ok(())
        }
        Err(error) => Err(workspace_error_to_apply_edit(error)),
    }
}

fn rename_workspace_edit_path(
    editor: &mut Editor,
    workspace_backend: &WorkspaceBackendHandle,
    old_path: &Path,
    new_path: &Path,
    options: Option<&lsp::RenameFileOptions>,
) -> Result<(), ApplyEditErrorKind> {
    let overwrite = options
        .and_then(|options| options.overwrite)
        .unwrap_or(false);
    let ignore_if_exists = options
        .and_then(|options| options.ignore_if_exists)
        .unwrap_or(false);

    if overwrite {
        let _ = futures_executor::block_on(workspace_backend.delete_path(new_path));
    }

    match futures_executor::block_on(workspace_backend.rename_path(old_path, new_path)) {
        Ok(stat) => {
            update_open_document_after_workspace_rename(editor, old_path, &stat);
            Ok(())
        }
        Err(error)
            if ignore_if_exists
                && workspace_error_has_io_kind(&error, std::io::ErrorKind::AlreadyExists) =>
        {
            Ok(())
        }
        Err(error) => Err(workspace_error_to_apply_edit(error)),
    }
}

fn apply_workspace_resource_op_with_backend(
    editor: &mut Editor,
    workspace_backend: &WorkspaceBackendHandle,
    project_directory: Option<&Path>,
    op: &lsp::ResourceOp,
) -> Result<(), ApplyEditErrorKind> {
    let workspace_identity = workspace_backend.identity();
    match op {
        lsp::ResourceOp::Create(op) => {
            let path =
                workspace_edit_path_from_url(&op.uri, project_directory, &workspace_identity)?;
            create_workspace_edit_file(workspace_backend, &path, op.options.as_ref())
        }
        lsp::ResourceOp::Delete(op) => {
            let path =
                workspace_edit_path_from_url(&op.uri, project_directory, &workspace_identity)?;
            delete_workspace_edit_path(workspace_backend, &path, op.options.as_ref())
        }
        lsp::ResourceOp::Rename(op) => {
            let old_path =
                workspace_edit_path_from_url(&op.old_uri, project_directory, &workspace_identity)?;
            let new_path =
                workspace_edit_path_from_url(&op.new_uri, project_directory, &workspace_identity)?;
            rename_workspace_edit_path(
                editor,
                workspace_backend,
                &old_path,
                &new_path,
                op.options.as_ref(),
            )
        }
    }
}

fn apply_workspace_edit_with_backend(
    editor: &mut Editor,
    workspace_backend: &WorkspaceBackendHandle,
    project_directory: Option<&Path>,
    offset_encoding: OffsetEncoding,
    workspace_edit: &lsp::WorkspaceEdit,
) -> Result<(), ApplyEditError> {
    if matches!(workspace_backend.identity(), WorkspaceIdentity::Local) {
        return editor.apply_workspace_edit(offset_encoding, workspace_edit);
    }

    if let Some(ref document_changes) = workspace_edit.document_changes {
        match document_changes {
            lsp::DocumentChanges::Edits(document_edits) => {
                for (i, document_edit) in document_edits.iter().enumerate() {
                    let edits = document_edit
                        .edits
                        .iter()
                        .map(|edit| match edit {
                            lsp::OneOf::Left(text_edit) => text_edit,
                            lsp::OneOf::Right(annotated_text_edit) => {
                                &annotated_text_edit.text_edit
                            }
                        })
                        .cloned()
                        .collect();
                    apply_workspace_text_edits_with_backend(
                        editor,
                        workspace_backend,
                        project_directory,
                        &document_edit.text_document.uri,
                        document_edit.text_document.version,
                        edits,
                        offset_encoding,
                    )
                    .map_err(|kind| ApplyEditError {
                        kind,
                        failed_change_idx: i,
                    })?;
                }
            }
            lsp::DocumentChanges::Operations(operations) => {
                for (i, operation) in operations.iter().enumerate() {
                    match operation {
                        lsp::DocumentChangeOperation::Op(op) => {
                            apply_workspace_resource_op_with_backend(
                                editor,
                                workspace_backend,
                                project_directory,
                                op,
                            )
                            .map_err(|kind| ApplyEditError {
                                kind,
                                failed_change_idx: i,
                            })?;
                        }
                        lsp::DocumentChangeOperation::Edit(document_edit) => {
                            let edits = document_edit
                                .edits
                                .iter()
                                .map(|edit| match edit {
                                    lsp::OneOf::Left(text_edit) => text_edit,
                                    lsp::OneOf::Right(annotated_text_edit) => {
                                        &annotated_text_edit.text_edit
                                    }
                                })
                                .cloned()
                                .collect();
                            apply_workspace_text_edits_with_backend(
                                editor,
                                workspace_backend,
                                project_directory,
                                &document_edit.text_document.uri,
                                document_edit.text_document.version,
                                edits,
                                offset_encoding,
                            )
                            .map_err(|kind| ApplyEditError {
                                kind,
                                failed_change_idx: i,
                            })?;
                        }
                    }
                }
            }
        }

        return Ok(());
    }

    if let Some(ref changes) = workspace_edit.changes {
        for (i, (uri, text_edits)) in changes.iter().enumerate() {
            apply_workspace_text_edits_with_backend(
                editor,
                workspace_backend,
                project_directory,
                uri,
                None,
                text_edits.to_vec(),
                offset_encoding,
            )
            .map_err(|kind| ApplyEditError {
                kind,
                failed_change_idx: i,
            })?;
        }
    }

    Ok(())
}

pub(super) struct PendingLspWorkspaceEdit {
    server_id: LanguageServerId,
    request_id: helix_lsp::jsonrpc::Id,
    offset_encoding: OffsetEncoding,
    operations: VecDeque<LspWorkspaceEditOperation>,
    operation_index: usize,
    active: Option<BoxFuture<'static, Result<LspWorkspaceEditAsyncResult, ApplyEditErrorKind>>>,
}

enum LspWorkspaceEditOperation {
    Text {
        uri: lsp::Url,
        version: Option<i32>,
        edits: Vec<lsp::TextEdit>,
    },
    Resource(lsp::ResourceOp),
}

enum LspWorkspaceEditAsyncResult {
    DocumentRead {
        document_read: WorkspaceDocumentRead,
        version: Option<i32>,
        edits: Vec<lsp::TextEdit>,
    },
    ResourceCreated,
    ResourceDeleted,
    ResourceRenamed {
        old_path: PathBuf,
        stat: FileStat,
    },
}

impl Application {
    pub(crate) fn apply_lsp_workspace_edit(
        &mut self,
        offset_encoding: OffsetEncoding,
        workspace_edit: &lsp::WorkspaceEdit,
    ) -> Result<(), ApplyEditError> {
        apply_workspace_edit_with_backend(
            &mut self.editor,
            &self.workspace_backend,
            self.project_directory.as_deref(),
            offset_encoding,
            workspace_edit,
        )
    }

    pub(super) fn enqueue_lsp_workspace_edit(
        &mut self,
        server_id: LanguageServerId,
        request_id: helix_lsp::jsonrpc::Id,
        offset_encoding: OffsetEncoding,
        workspace_edit: lsp::WorkspaceEdit,
    ) {
        self.pending_lsp_workspace_edits
            .push_back(PendingLspWorkspaceEdit {
                server_id,
                request_id,
                offset_encoding,
                operations: lsp_workspace_edit_operations(workspace_edit),
                operation_index: 0,
                active: None,
            });
    }

    fn reply_lsp_workspace_edit(
        &self,
        pending: PendingLspWorkspaceEdit,
        result: Result<(), ApplyEditErrorKind>,
    ) {
        let reply = Ok(serde_json::json!({
            "applied": result.is_ok(),
            "failureReason": result
                .as_ref()
                .err()
                .map(|error| format!("workspace edit failed: {error:?}")),
            "failedChange": result.is_err().then_some(pending.operation_index),
        }));
        if let Some(language_server) = self.editor.language_server_by_id(pending.server_id)
            && let Err(error) = language_server.reply(pending.request_id, reply)
        {
            error!(
                server_id = ?pending.server_id,
                %error,
                "Failed to send deferred workspace edit reply"
            );
        }
    }

    pub(super) fn poll_pending_lsp_workspace_edits(
        &mut self,
        task_cx: &mut TaskContext<'_>,
    ) -> bool {
        let Some(mut pending) = self.pending_lsp_workspace_edits.pop_front() else {
            return false;
        };
        let mut progressed = false;

        for _ in 0..MAINTENANCE_LSP_COMMAND_BATCH {
            if let Some(active) = pending.active.as_mut() {
                match active.as_mut().poll(task_cx) {
                    Poll::Pending => {
                        self.pending_lsp_workspace_edits.push_front(pending);
                        return progressed;
                    }
                    Poll::Ready(result) => {
                        pending.active = None;
                        progressed = true;
                        match result {
                            Ok(LspWorkspaceEditAsyncResult::DocumentRead {
                                document_read,
                                version,
                                edits,
                            }) => {
                                let action = if self.editor.tree.views().count() == 0 {
                                    helix_view::editor::Action::VerticalSplit
                                } else {
                                    helix_view::editor::Action::Load
                                };
                                let result = open_workspace_document_from_read(
                                    &mut self.editor,
                                    document_read,
                                    action,
                                )
                                .map_err(|error| apply_edit_io_error(error.to_string()))
                                .and_then(|doc_id| {
                                    apply_workspace_text_edits_to_open_document(
                                        &mut self.editor,
                                        doc_id,
                                        version,
                                        edits,
                                        pending.offset_encoding,
                                    )
                                });
                                if let Err(error) = result {
                                    self.reply_lsp_workspace_edit(pending, Err(error));
                                    return true;
                                }
                            }
                            Ok(LspWorkspaceEditAsyncResult::ResourceCreated)
                            | Ok(LspWorkspaceEditAsyncResult::ResourceDeleted) => {}
                            Ok(LspWorkspaceEditAsyncResult::ResourceRenamed { old_path, stat }) => {
                                update_open_document_after_workspace_rename(
                                    &mut self.editor,
                                    &old_path,
                                    &stat,
                                )
                            }
                            Err(error) => {
                                self.reply_lsp_workspace_edit(pending, Err(error));
                                return true;
                            }
                        }
                        pending.operation_index += 1;
                        continue;
                    }
                }
            }

            let Some(operation) = pending.operations.pop_front() else {
                self.reply_lsp_workspace_edit(pending, Ok(()));
                return true;
            };

            let workspace_identity = self.workspace_backend.identity();
            match operation {
                LspWorkspaceEditOperation::Text {
                    uri,
                    version,
                    edits,
                } => {
                    let path = match workspace_edit_path_from_url(
                        &uri,
                        self.project_directory.as_deref(),
                        &workspace_identity,
                    ) {
                        Ok(path) => path,
                        Err(error) => {
                            self.reply_lsp_workspace_edit(pending, Err(error));
                            return true;
                        }
                    };
                    if let Some(doc_id) = self.editor.document_by_path(&path).map(Document::id) {
                        match apply_workspace_text_edits_to_open_document(
                            &mut self.editor,
                            doc_id,
                            version,
                            edits,
                            pending.offset_encoding,
                        ) {
                            Ok(()) => {
                                pending.operation_index += 1;
                                progressed = true;
                            }
                            Err(error) => {
                                self.reply_lsp_workspace_edit(pending, Err(error));
                                return true;
                            }
                        }
                    } else {
                        let backend = self.workspace_backend.clone();
                        pending.active = Some(
                            async move {
                                let document_read = read_workspace_document(backend, path)
                                    .await
                                    .map_err(|error| apply_edit_io_error(error.to_string()))?;
                                Ok(LspWorkspaceEditAsyncResult::DocumentRead {
                                    document_read,
                                    version,
                                    edits,
                                })
                            }
                            .boxed(),
                        );
                        progressed = true;
                    }
                }
                LspWorkspaceEditOperation::Resource(resource_op) => {
                    let backend = self.workspace_backend.clone();
                    let project_directory = self.project_directory.clone();
                    pending.active = Some(
                        async move {
                            match resource_op {
                                lsp::ResourceOp::Create(op) => {
                                    let path = workspace_edit_path_from_url(
                                        &op.uri,
                                        project_directory.as_deref(),
                                        &backend.identity(),
                                    )?;
                                    let overwrite = op
                                        .options
                                        .as_ref()
                                        .and_then(|options| options.overwrite)
                                        .unwrap_or(false);
                                    let ignore_if_exists = op
                                        .options
                                        .as_ref()
                                        .and_then(|options| options.ignore_if_exists)
                                        .unwrap_or(false);
                                    let result = if overwrite {
                                        backend
                                            .write_file(
                                                &path,
                                                &[],
                                                WriteOptions {
                                                    create_parent_dirs: true,
                                                    expected_modified: None,
                                                    expected_version: None,
                                                },
                                            )
                                            .await
                                            .map(|_| ())
                                    } else {
                                        backend.create_file(&path).await.map(|_| ())
                                    };
                                    match result {
                                        Ok(()) => Ok(LspWorkspaceEditAsyncResult::ResourceCreated),
                                        Err(error)
                                            if ignore_if_exists
                                                && workspace_error_has_io_kind(
                                                    &error,
                                                    std::io::ErrorKind::AlreadyExists,
                                                ) =>
                                        {
                                            Ok(LspWorkspaceEditAsyncResult::ResourceCreated)
                                        }
                                        Err(error) => Err(workspace_error_to_apply_edit(error)),
                                    }
                                }
                                lsp::ResourceOp::Delete(op) => {
                                    let path = workspace_edit_path_from_url(
                                        &op.uri,
                                        project_directory.as_deref(),
                                        &backend.identity(),
                                    )?;
                                    let ignore_if_not_exists = op
                                        .options
                                        .as_ref()
                                        .and_then(|options| options.ignore_if_not_exists)
                                        .unwrap_or(false);
                                    match backend.delete_path(&path).await {
                                        Ok(_) => Ok(LspWorkspaceEditAsyncResult::ResourceDeleted),
                                        Err(error)
                                            if ignore_if_not_exists
                                                && workspace_error_has_io_kind(
                                                    &error,
                                                    std::io::ErrorKind::NotFound,
                                                ) =>
                                        {
                                            Ok(LspWorkspaceEditAsyncResult::ResourceDeleted)
                                        }
                                        Err(error) => Err(workspace_error_to_apply_edit(error)),
                                    }
                                }
                                lsp::ResourceOp::Rename(op) => {
                                    let old_path = workspace_edit_path_from_url(
                                        &op.old_uri,
                                        project_directory.as_deref(),
                                        &backend.identity(),
                                    )?;
                                    let new_path = workspace_edit_path_from_url(
                                        &op.new_uri,
                                        project_directory.as_deref(),
                                        &backend.identity(),
                                    )?;
                                    let overwrite = op
                                        .options
                                        .as_ref()
                                        .and_then(|options| options.overwrite)
                                        .unwrap_or(false);
                                    let ignore_if_exists = op
                                        .options
                                        .as_ref()
                                        .and_then(|options| options.ignore_if_exists)
                                        .unwrap_or(false);
                                    if overwrite {
                                        let _ = backend.delete_path(&new_path).await;
                                    }
                                    match backend.rename_path(&old_path, &new_path).await {
                                        Ok(stat) => {
                                            Ok(LspWorkspaceEditAsyncResult::ResourceRenamed {
                                                old_path,
                                                stat,
                                            })
                                        }
                                        Err(error)
                                            if ignore_if_exists
                                                && workspace_error_has_io_kind(
                                                    &error,
                                                    std::io::ErrorKind::AlreadyExists,
                                                ) =>
                                        {
                                            Ok(LspWorkspaceEditAsyncResult::ResourceCreated)
                                        }
                                        Err(error) => Err(workspace_error_to_apply_edit(error)),
                                    }
                                }
                            }
                        }
                        .boxed(),
                    );
                    progressed = true;
                }
            }
        }

        self.pending_lsp_workspace_edits.push_front(pending);
        progressed
    }
}
